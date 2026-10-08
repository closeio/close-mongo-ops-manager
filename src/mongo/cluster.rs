//! Per-node mode (`--all-nodes`): every cluster member is polled through its
//! own direct connection (as Percona PMM does), so operations running on
//! secondaries and config servers are listed too.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures::future::{BoxFuture, FutureExt, Shared, join_all, try_join};
use mongodb::Client;
use mongodb::bson::{Document, doc};

use super::call::{CallError, admin_aggregate, admin_command};
use super::options::{DirectClients, Users};
use super::parse::{OpContext, parse_operations};
use super::pipeline::LISTING_BATCH_SIZE;
use super::topology::{Hello, normalize_address, parse_replica_set_hosts};
use crate::model::{
    Deployment, MAX_OPERATIONS, NodeHealth, NodeRole, NodeStatus, OpSource, Operation,
};

/// Members are rediscovered when the last discovery is older than this.
pub(crate) const REDISCOVERY_INTERVAL: Duration = Duration::from_secs(30);

/// A member that could not be polled is reported as failed right away, and
/// polled again in the background, during this long: a dead member must not
/// slow every refresh down by the timeout.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Shard name of the config server replica set.
pub(crate) const CONFIG_SHARD: &str = "config";

/// A cluster member polled directly.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    /// `host:port`, as named by the replica set configuration.
    pub address: String,
    /// Shard / replica set name, [`CONFIG_SHARD`] for config servers.
    pub shard: String,
    /// The replica set name the member's client requires, when known.
    pub set_name: Option<String>,
    pub role: NodeRole,
    /// The direct client, or why it could not be created.
    pub client: Result<Client, String>,
}

type InFlight = Shared<BoxFuture<'static, ()>>;

/// The members of the cluster and their direct clients.
pub(crate) struct NodePool {
    inner: Arc<Inner>,
}

struct Inner {
    main: Client,
    deployment: Deployment,
    direct: DirectClients,
    timeout: Duration,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    members: Vec<Member>,
    /// When the last discovery finished (successfully or not).
    last_discovery: Option<Instant>,
    in_flight: Option<InFlight>,
    /// Why the last discovery failed.
    discovery_error: Option<String>,
    /// Problems found by the last successful discovery.
    discovery_warnings: Vec<String>,
    /// Shard of every member ever discovered, by address.
    shards: HashMap<String, String>,
    /// Members whose last poll failed, by address.
    failures: HashMap<String, Failure>,
    shut_down: bool,
}

struct Failure {
    error: String,
    /// When to poll the member again.
    retry_at: Instant,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl NodePool {
    pub fn new(
        main: Client,
        deployment: Deployment,
        direct: DirectClients,
        timeout: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                main,
                deployment,
                direct,
                timeout,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Starts a discovery when one is due (or `force`d) and returns the
    /// discovery in flight, if any.
    ///
    /// Discoveries run in their own task: they complete (and update the pool
    /// in one step) even if whoever started them is dropped.
    fn ensure_discovery(&self, force: bool) -> Option<InFlight> {
        let mut state = self.inner.state();
        if let Some(in_flight) = &state.in_flight {
            return Some(in_flight.clone());
        }
        let due = force
            || state
                .last_discovery
                .is_none_or(|t| t.elapsed() >= REDISCOVERY_INTERVAL);
        if state.shut_down || !due {
            return None;
        }
        let task = tokio::spawn(run_discovery(self.inner.clone()));
        let in_flight = async move {
            // The task clears `in_flight` itself, even if it panics.
            let _ = task.await;
        }
        .boxed()
        .shared();
        state.in_flight = Some(in_flight.clone());
        Some(in_flight)
    }

    /// Discovers the members now and waits for the result.
    pub async fn discover_now(&self) {
        if let Some(in_flight) = self.ensure_discovery(true) {
            in_flight.await;
        }
    }

    /// The members to poll. Starts a rediscovery in the background when one
    /// is due.
    pub fn members_to_poll(&self) -> Vec<Member> {
        self.ensure_discovery(false);
        self.members()
    }

    pub fn members(&self) -> Vec<Member> {
        self.inner.state().members.clone()
    }

    /// Discovery problems that hold now. Reported with every snapshot for
    /// as long as they hold.
    pub fn warnings(&self) -> Vec<String> {
        let state = self.inner.state();
        state
            .discovery_error
            .iter()
            .map(|e| format!("could not discover the cluster members: {e}"))
            .chain(state.discovery_warnings.iter().cloned())
            .collect()
    }

    /// Members whose last poll failed.
    pub fn failing(&self) -> HashSet<String> {
        self.inner.state().failures.keys().cloned().collect()
    }

    /// The shard of a member, also after it left the cluster.
    pub fn shard_of(&self, address: &str) -> Option<String> {
        self.inner.state().shards.get(address).cloned()
    }

    /// The replica set name required of the member at `address`, if known.
    pub fn set_name_of(&self, address: &str) -> Option<String> {
        self.inner
            .state()
            .members
            .iter()
            .find(|m| m.address == address)
            .and_then(|m| m.set_name.clone())
    }

    /// The replica set name of the members of `shard`, if known.
    pub fn shard_set_name(&self, shard: &str) -> Option<String> {
        self.inner
            .state()
            .members
            .iter()
            .find(|m| m.shard == shard && m.set_name.is_some())
            .and_then(|m| m.set_name.clone())
    }

    /// The addresses of the members of `shard`.
    pub fn addresses_of(&self, shard: &str) -> Vec<String> {
        self.inner
            .state()
            .members
            .iter()
            .filter(|m| m.shard == shard)
            .map(|m| m.address.clone())
            .collect()
    }

    /// Polls every member with `pipeline`.
    ///
    /// Each poll runs in its own task and records its outcome in the pool, so
    /// the outcome is kept even if the fetch is dropped. Members whose last
    /// poll failed are reported with that error right away, and polled again
    /// in the background every [`RETRY_INTERVAL`].
    pub async fn poll(&self, members: &[Member], pipeline: &[Document]) -> Vec<MemberPoll> {
        enum Poll {
            /// Failing: reported with its last error without waiting.
            Failing(String),
            Running(tokio::task::JoinHandle<MemberPoll>),
        }
        let now = Instant::now();
        let polls = members.iter().map(|member| {
            let failure = {
                let mut state = self.inner.state();
                state.failures.get_mut(&member.address).map(|failure| {
                    let retry = now >= failure.retry_at;
                    if retry {
                        failure.retry_at = now + RETRY_INTERVAL;
                    }
                    (failure.error.clone(), retry)
                })
            };
            let spawn = || {
                tokio::spawn(poll_member(
                    self.inner.clone(),
                    member.clone(),
                    pipeline.to_vec(),
                ))
            };
            let poll = match failure {
                Some((error, retry)) => {
                    if retry {
                        // In the background: its outcome is recorded.
                        drop(spawn());
                    }
                    Poll::Failing(error)
                }
                None => Poll::Running(spawn()),
            };
            async move {
                match poll {
                    Poll::Failing(error) => MemberPoll::failed(member, error),
                    Poll::Running(task) => task.await.unwrap_or_else(|e| {
                        MemberPoll::failed(member, format!("polling failed: {e}"))
                    }),
                }
            }
        });
        join_all(polls).await
    }

    /// The direct client of a known member, if it uses `users`.
    pub fn client_for(&self, address: &str, users: Users) -> Option<Client> {
        self.inner
            .state()
            .members
            .iter()
            .find(|m| m.address == address && users_of(&m.shard) == users)
            .and_then(|m| m.client.clone().ok())
    }

    /// Takes every client out of the pool, for shutdown. Later discoveries
    /// shut their clients down instead of keeping them.
    pub fn take_clients(&self) -> Vec<Client> {
        let mut state = self.inner.state();
        state.shut_down = true;
        state
            .members
            .drain(..)
            .filter_map(|m| m.client.ok())
            .collect()
    }
}

/// Clears the in-flight marker if the discovery task ends without applying
/// its result (panic or runtime shutdown).
struct DiscoveryGuard {
    inner: Arc<Inner>,
    applied: bool,
}

impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        if !self.applied {
            let mut state = self.inner.state();
            state.in_flight = None;
            state.last_discovery = Some(Instant::now());
        }
    }
}

async fn run_discovery(inner: Arc<Inner>) {
    let mut guard = DiscoveryGuard {
        inner: inner.clone(),
        applied: false,
    };
    let existing: HashMap<String, Client> = inner
        .state()
        .members
        .iter()
        .filter_map(|m| Some((m.address.clone(), m.client.clone().ok()?)))
        .collect();
    let result = discover(&inner, &existing).await;
    let stale = apply_discovery(&inner, result);
    guard.applied = true;
    shutdown_in_background(stale, inner.timeout);
}

/// A member found by a discovery.
struct Found {
    member: Member,
    /// The client was created by this discovery (not taken from the pool).
    created: bool,
}

struct Discovered {
    members: Vec<Found>,
    warnings: Vec<String>,
}

/// Applies a discovery to the pool. Returns the clients to shut down.
fn apply_discovery(inner: &Inner, result: Result<Discovered, String>) -> Vec<Client> {
    let mut state = inner.state();
    state.in_flight = None;
    state.last_discovery = Some(Instant::now());
    let discovered = match result {
        Ok(discovered) => discovered,
        Err(error) => {
            // The members of the last successful discovery stay.
            if state.discovery_error.as_ref() != Some(&error) {
                log::warn!("Could not discover the cluster members: {error}");
            }
            state.discovery_error = Some(error);
            return Vec::new();
        }
    };
    if state.discovery_error.take().is_some() {
        log::info!("Discovered the cluster members again");
    }
    if state.discovery_warnings != discovered.warnings {
        for warning in &discovered.warnings {
            log::warn!("{warning}");
        }
        state.discovery_warnings = discovered.warnings;
    }

    let mut stale = Vec::new();
    if state.shut_down {
        stale.extend(
            discovered
                .members
                .into_iter()
                .filter(|f| f.created)
                .filter_map(|f| f.member.client.ok()),
        );
        return stale;
    }

    let mut current: HashMap<String, Member> = state
        .members
        .drain(..)
        .map(|m| (m.address.clone(), m))
        .collect();
    let mut added = Vec::new();
    let mut members = Vec::with_capacity(discovered.members.len());
    for Found {
        mut member,
        created,
    } in discovered.members
    {
        state
            .shards
            .insert(member.address.clone(), member.shard.clone());
        match current.remove(&member.address) {
            Some(old) => {
                // Unreachable now: keep the last known role.
                if member.role == NodeRole::Unknown {
                    member.role = old.role;
                }
                // Keep the pooled client.
                if old.client.is_ok()
                    && let Ok(new) = std::mem::replace(&mut member.client, old.client)
                    && created
                {
                    stale.push(new);
                }
            }
            None => added.push(member.address.clone()),
        }
        members.push(member);
    }
    let removed: Vec<String> = current.keys().cloned().collect();
    stale.extend(current.into_values().filter_map(|m| m.client.ok()));
    state
        .failures
        .retain(|address, _| members.iter().any(|m| &m.address == address));
    if !added.is_empty() || !removed.is_empty() {
        log::info!(
            "Cluster members: {} (added: {}; removed: {})",
            members.len(),
            list_or_none(&added),
            list_or_none(&removed)
        );
    }
    state.members = members;
    stale
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

/// Shuts clients down without waiting.
pub(crate) fn shutdown_in_background(clients: Vec<Client>, timeout: Duration) {
    if clients.is_empty() {
        return;
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            join_all(clients.into_iter().map(|client| async move {
                let _ = tokio::time::timeout(timeout, client.shutdown().immediate(true)).await;
            }))
            .await;
        });
    }
}

/// A member to probe, with the shard it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seed {
    address: String,
    shard: String,
    /// The replica set the member must belong to, when known: its direct
    /// client refuses a server of another replica set.
    set_name: Option<String>,
}

/// Seeds from a replica set connection string such as `"rs0/h1:1,h2:2"`.
fn seeds_from_hosts(shard: &str, hosts: &str) -> Vec<Seed> {
    let (set_name, hosts) = parse_replica_set_hosts(hosts);
    hosts
        .iter()
        .filter_map(|h| normalize_address(h))
        .map(|address| Seed {
            address,
            shard: shard.to_owned(),
            set_name: set_name.clone(),
        })
        .collect()
}

/// The addresses of the members of `shard` in a `listShards` response, and
/// the shard's replica set name.
pub(crate) fn shard_hosts(response: &Document, shard: &str) -> (Vec<String>, Option<String>) {
    let seeds: Vec<Seed> = seeds_from_list_shards(response)
        .into_iter()
        .filter(|s| s.shard == shard)
        .collect();
    let set_name = seeds.first().and_then(|s| s.set_name.clone());
    (seeds.into_iter().map(|s| s.address).collect(), set_name)
}

/// Seeds from a `listShards` response.
fn seeds_from_list_shards(response: &Document) -> Vec<Seed> {
    let Ok(shards) = response.get_array("shards") else {
        return Vec::new();
    };
    shards
        .iter()
        .filter_map(|s| s.as_document())
        .filter_map(|s| Some((s.get_str("_id").ok()?, s.get_str("host").ok()?)))
        .flat_map(|(name, hosts)| seeds_from_hosts(name, hosts))
        .collect()
}

/// The config server connection string from `serverStatus` (`sharding`
/// section) or `getShardMap` (`map.config`).
fn config_servers_from(response: &Document) -> Option<&str> {
    response
        .get_document("sharding")
        .ok()
        .and_then(|s| s.get_str("configsvrConnectionString").ok())
        .or_else(|| {
            response
                .get_document("map")
                .ok()
                .and_then(|m| m.get_str(CONFIG_SHARD).ok())
        })
        .filter(|s| !s.trim().is_empty())
}

/// What probing one member returned.
struct Probe {
    seed: Seed,
    client: Result<Client, String>,
    created: bool,
    hello: Option<Hello>,
}

/// Turns probes into members: drops arbiters (by their own `hello` or as
/// reported by another member) and duplicates.
fn members_from_probes(probes: Vec<Probe>) -> (Vec<Found>, Vec<Client>) {
    let arbiters: HashSet<String> = probes
        .iter()
        .filter_map(|p| p.hello.as_ref())
        .flat_map(|h| h.arbiters.iter())
        .filter_map(|a| normalize_address(a))
        .collect();
    let mut seen = HashSet::new();
    let mut members = Vec::new();
    let mut dropped = Vec::new();
    for probe in probes {
        let role = probe.hello.as_ref().map_or(NodeRole::Unknown, Hello::role);
        if role == NodeRole::Arbiter
            || arbiters.contains(&probe.seed.address)
            || !seen.insert(probe.seed.address.clone())
        {
            if probe.created
                && let Ok(client) = probe.client
            {
                dropped.push(client);
            }
            continue;
        }
        members.push(Found {
            member: Member {
                address: probe.seed.address,
                shard: probe.seed.shard,
                set_name: probe.seed.set_name,
                role,
                client: probe.client,
            },
            created: probe.created,
        });
    }
    members.sort_by(|a, b| member_order(&a.member).cmp(&member_order(&b.member)));
    (members, dropped)
}

fn member_order(member: &Member) -> (bool, &str, &str) {
    (member.shard == CONFIG_SHARD, &member.shard, &member.address)
}

/// Members reported by probed members that were not probed yet (nor known
/// under another name through `hello.me`), with the reporter's shard.
fn unprobed_members(probes: &[Probe]) -> Vec<Seed> {
    let mut known: HashSet<String> = probes.iter().map(|p| p.seed.address.clone()).collect();
    known.extend(
        probes
            .iter()
            .filter_map(|p| p.hello.as_ref()?.me.as_deref())
            .filter_map(normalize_address),
    );
    let mut seeds = Vec::new();
    for probe in probes {
        let Some(hello) = &probe.hello else { continue };
        for address in hello.data_members().filter_map(normalize_address) {
            if known.insert(address.clone()) {
                seeds.push(Seed {
                    address,
                    shard: probe.seed.shard.clone(),
                    set_name: probe
                        .seed
                        .set_name
                        .clone()
                        .or_else(|| hello.set_name.clone()),
                });
            }
        }
    }
    seeds
}

async fn discover(inner: &Inner, existing: &HashMap<String, Client>) -> Result<Discovered, String> {
    let (seeds, warnings) = discovery_seeds(inner).await?;
    let mut unique = HashSet::new();
    let seeds: Vec<Seed> = seeds
        .into_iter()
        .filter(|s| unique.insert(s.address.clone()))
        .collect();

    let mut probes = probe_all(inner, existing, seeds).await;
    let more = unprobed_members(&probes);
    if !more.is_empty() {
        probes.extend(probe_all(inner, existing, more).await);
    }
    let (members, dropped) = members_from_probes(probes);
    shutdown_in_background(dropped, inner.timeout);
    Ok(Discovered { members, warnings })
}

/// The members to probe first: from `listShards` and the config server
/// connection string through mongos, or from the main `hello` on a replica
/// set.
async fn discovery_seeds(inner: &Inner) -> Result<(Vec<Seed>, Vec<String>), String> {
    let timeout = inner.timeout;
    let mut warnings = Vec::new();
    let seeds = match &inner.deployment {
        Deployment::Sharded => {
            let shards =
                admin_command(&inner.main, "listShards", doc! { "listShards": 1 }, timeout)
                    .await
                    .map_err(|e| e.to_string())?;
            let mut seeds = seeds_from_list_shards(&shards);
            match config_server_hosts(inner).await {
                Ok(hosts) => seeds.extend(seeds_from_hosts(CONFIG_SHARD, &hosts)),
                Err(error) => warnings.push(format!("could not find the config servers: {error}")),
            }
            seeds
        }
        Deployment::ReplicaSet { name } => {
            let hello = admin_command(&inner.main, "hello", doc! { "hello": 1 }, timeout)
                .await
                .map_err(|e| e.to_string())?;
            Hello::parse(&hello)
                .data_members()
                .filter_map(normalize_address)
                .map(|address| Seed {
                    address,
                    shard: name.clone(),
                    set_name: Some(name.clone()),
                })
                .collect()
        }
        Deployment::Standalone => Vec::new(),
    };
    Ok((seeds, warnings))
}

async fn config_server_hosts(inner: &Inner) -> Result<String, String> {
    let status = admin_command(
        &inner.main,
        "serverStatus",
        doc! { "serverStatus": 1 },
        inner.timeout,
    )
    .await;
    if let Some(hosts) = status.as_ref().ok().and_then(config_servers_from) {
        return Ok(hosts.to_owned());
    }
    let map = admin_command(
        &inner.main,
        "getShardMap",
        doc! { "getShardMap": 1 },
        inner.timeout,
    )
    .await
    .map_err(|e| e.to_string())?;
    config_servers_from(&map)
        .map(str::to_owned)
        .ok_or_else(|| "neither serverStatus nor getShardMap report them".to_owned())
}

/// Sends `hello` to every seed through its direct client.
async fn probe_all(
    inner: &Inner,
    existing: &HashMap<String, Client>,
    seeds: Vec<Seed>,
) -> Vec<Probe> {
    join_all(seeds.into_iter().map(|seed| async move {
        let (client, created) = match existing.get(&seed.address) {
            Some(client) => (Ok(client.clone()), false),
            None => (
                inner.direct.client(
                    &seed.address,
                    users_of(&seed.shard),
                    seed.set_name.as_deref(),
                ),
                true,
            ),
        };
        let hello = match &client {
            Ok(client) => {
                match admin_command(client, "hello", doc! { "hello": 1 }, inner.timeout).await {
                    Ok(response) => Some(Hello::parse(&response)),
                    Err(error) => {
                        log::debug!("Could not reach cluster member {}: {error}", seed.address);
                        None
                    }
                }
            }
            Err(_) => None,
        };
        Probe {
            seed,
            client,
            created,
            hello,
        }
    }))
    .await
}

/// Operations listed on one member.
pub(crate) struct MemberPoll {
    pub outcome: PollOutcome,
    pub ops: Vec<Operation>,
    /// More than [`MAX_OPERATIONS`] matched.
    pub truncated: bool,
}

impl MemberPoll {
    fn failed(member: &Member, error: String) -> Self {
        Self {
            outcome: PollOutcome {
                address: member.address.clone(),
                shard: member.shard.clone(),
                role: member.role,
                result: Err(error),
                latency: Duration::ZERO,
            },
            ops: Vec::new(),
            truncated: false,
        }
    }
}

/// Polls one member and records the outcome (role, failure) in the pool.
async fn poll_member(inner: Arc<Inner>, member: Member, pipeline: Vec<Document>) -> MemberPoll {
    let hints = Hints {
        sharded: inner.deployment == Deployment::Sharded,
        node_credential: inner.direct.has_node_credential(),
    };
    let poll = poll_once(&member, pipeline, inner.timeout, hints).await;
    let outcome = &poll.outcome;
    let mut state = inner.state();
    let Some(known) = state
        .members
        .iter_mut()
        .find(|m| m.address == outcome.address)
    else {
        // Removed by a discovery in the meantime.
        return poll;
    };
    known.role = outcome.role;
    match &outcome.result {
        Ok(_) => {
            if state.failures.remove(&outcome.address).is_some() {
                log::info!("Cluster member {} is back", outcome.address);
            }
        }
        Err(error) => {
            if !state.failures.contains_key(&outcome.address) {
                log::warn!("Could not poll cluster member {}: {error}", outcome.address);
            }
            state.failures.insert(
                outcome.address.clone(),
                Failure {
                    error: error.clone(),
                    retry_at: Instant::now() + RETRY_INTERVAL,
                },
            );
        }
    }
    drop(state);
    poll
}

/// Lists the operations of one member, and gets its current role.
async fn poll_once(
    member: &Member,
    pipeline: Vec<Document>,
    timeout: Duration,
    hints: Hints,
) -> MemberPoll {
    let client = match &member.client {
        Ok(client) => client,
        Err(error) => return MemberPoll::failed(member, error.clone()),
    };
    let listing = async {
        let started = Instant::now();
        admin_aggregate(
            client,
            "listing operations",
            pipeline,
            LISTING_BATCH_SIZE,
            timeout,
        )
        .await
        .map(|docs| (docs, started.elapsed()))
    };
    // The role is refreshed along the way; a failed listing does not wait
    // for it.
    let hello = async {
        Ok(admin_command(client, "hello", doc! { "hello": 1 }, timeout)
            .await
            .ok())
    };
    match try_join(listing, hello).await {
        Ok(((docs, latency), hello)) => {
            let role = hello.map_or(member.role, |h| Hello::parse(&h).role());
            let source = OpSource::Node {
                address: member.address.clone(),
            };
            let ctx = OpContext {
                source: &source,
                shard: Some(&member.shard),
                node_role: Some(role),
                default_host: Some(&member.address),
            };
            let truncated = docs.len() > MAX_OPERATIONS;
            let ops = parse_operations(docs, &ctx);
            MemberPoll {
                outcome: PollOutcome {
                    address: member.address.clone(),
                    shard: member.shard.clone(),
                    role,
                    result: Ok(ops.len()),
                    latency,
                },
                ops,
                truncated,
            }
        }
        Err(error) => {
            let message = member_error(&error, &member.shard, hints);
            MemberPoll::failed(member, message)
        }
    }
}

/// The credential for the members of `shard`: users created through mongos
/// are stored on the config servers.
pub(crate) fn users_of(shard: &str) -> Users {
    if shard == CONFIG_SHARD {
        Users::Cluster
    } else {
        Users::Node
    }
}

/// What the errors of members are explained with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Hints {
    pub sharded: bool,
    /// `--node-username` was given.
    pub node_credential: bool,
}

/// The error of a member that could not be polled, with a hint for
/// authentication failures on shard members.
pub(crate) fn member_error(error: &CallError, shard: &str, hints: Hints) -> String {
    let mut message = error.to_string();
    if error.is_auth_failure() && hints.sharded && shard != CONFIG_SHARD {
        message.push_str(if hints.node_credential {
            " (direct connections to shard members need a user of the shard itself: \
             check --node-username/--node-password)"
        } else {
            " (users created through mongos are stored on the config servers; direct \
             connections to shard members need a shard-local user: use \
             --node-username/--node-password)"
        });
    }
    message
}

/// Result of polling one member.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PollOutcome {
    pub address: String,
    pub shard: String,
    pub role: NodeRole,
    pub result: Result<usize, String>,
    pub latency: Duration,
}

/// Shards (config servers excluded) without a successfully polled primary:
/// their primary's operations must be listed through mongos instead.
pub(crate) fn shards_needing_fallback(outcomes: &[PollOutcome]) -> Vec<String> {
    let shards: BTreeSet<&str> = outcomes
        .iter()
        .map(|o| o.shard.as_str())
        .filter(|s| *s != CONFIG_SHARD)
        .collect();
    shards
        .into_iter()
        .filter(|shard| {
            !outcomes
                .iter()
                .any(|o| o.shard == *shard && o.role == NodeRole::Primary && o.result.is_ok())
        })
        .map(str::to_owned)
        .collect()
}

/// The shards that will need the fallback, predicted before polling from
/// the members already known to fail (and their last known roles), so that
/// it can run along with the polls.
pub(crate) fn predicted_fallback_shards(
    members: &[Member],
    failing: &HashSet<String>,
) -> Vec<String> {
    let predicted: Vec<PollOutcome> = members
        .iter()
        .map(|m| PollOutcome {
            address: m.address.clone(),
            shard: m.shard.clone(),
            role: m.role,
            result: if failing.contains(&m.address) {
                Err(String::new())
            } else {
                Ok(0)
            },
            latency: Duration::ZERO,
        })
        .collect();
    shards_needing_fallback(&predicted)
}

/// Node statuses; failed members of `fallback_shards` (whose operations were
/// listed through mongos) are marked as such.
pub(crate) fn node_statuses(
    outcomes: &[PollOutcome],
    fallback_shards: &[String],
) -> Vec<NodeStatus> {
    outcomes
        .iter()
        .map(|o| NodeStatus {
            address: o.address.clone(),
            shard: Some(o.shard.clone()),
            role: o.role,
            health: match &o.result {
                Ok(operations) => NodeHealth::Ok {
                    operations: *operations,
                    latency: o.latency,
                },
                Err(error) if fallback_shards.contains(&o.shard) => NodeHealth::Fallback {
                    error: error.clone(),
                },
                Err(error) => NodeHealth::Failed {
                    error: error.clone(),
                },
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mongodb::options::ClientOptions;

    use super::*;
    use crate::mongo::options::MainOptions;

    /// A pool whose clients never connect (they are lazy).
    fn pool(deployment: Deployment) -> NodePool {
        let main = MainOptions {
            options: ClientOptions::default(),
            target: "localhost:27017".into(),
            load_balanced: false,
        };
        let direct = DirectClients::new(&main, None);
        let client = Client::with_options(ClientOptions::default()).unwrap();
        NodePool::new(client, deployment, direct, Duration::from_millis(50))
    }

    fn found(pool: &NodePool, address: &str, shard: &str, role: NodeRole) -> Found {
        Found {
            member: Member {
                address: address.into(),
                shard: shard.into(),
                set_name: None,
                role,
                client: pool.inner.direct.client(address, users_of(shard), None),
            },
            created: true,
        }
    }

    fn discovered(members: Vec<Found>) -> Result<Discovered, String> {
        Ok(Discovered {
            members,
            warnings: Vec::new(),
        })
    }

    fn summary(pool: &NodePool) -> Vec<(String, NodeRole)> {
        pool.members()
            .into_iter()
            .map(|m| (m.address, m.role))
            .collect()
    }

    #[tokio::test]
    async fn discoveries_reconcile_the_members() {
        let pool = pool(Deployment::Sharded);
        let inner = &pool.inner;
        let first = vec![
            found(&pool, "db1:27018", "shard01", NodeRole::Primary),
            found(&pool, "db2:27018", "shard01", NodeRole::Secondary),
        ];
        assert!(apply_discovery(inner, discovered(first)).is_empty());
        assert_eq!(
            summary(&pool),
            [
                ("db1:27018".to_owned(), NodeRole::Primary),
                ("db2:27018".to_owned(), NodeRole::Secondary)
            ]
        );
        inner.state().failures.insert(
            "db1:27018".into(),
            Failure {
                error: "down".into(),
                retry_at: Instant::now(),
            },
        );

        // db1 unreachable now, db2 gone, db3 new: the duplicate client for
        // db1 and db2's client are shut down.
        let second = vec![
            found(&pool, "db1:27018", "shard01", NodeRole::Unknown),
            found(&pool, "db3:27018", "shard01", NodeRole::Secondary),
        ];
        let stale = apply_discovery(inner, discovered(second));
        assert_eq!(stale.len(), 2);
        assert_eq!(
            summary(&pool),
            [
                ("db1:27018".to_owned(), NodeRole::Primary),
                ("db3:27018".to_owned(), NodeRole::Secondary)
            ]
        );
        assert!(inner.state().failures.contains_key("db1:27018"));

        let third = vec![found(&pool, "db3:27018", "shard01", NodeRole::Primary)];
        apply_discovery(inner, discovered(third));
        assert!(inner.state().failures.is_empty());
        assert!(pool.client_for("db3:27018", Users::Node).is_some());
        assert!(pool.client_for("db1:27018", Users::Node).is_none());
    }

    #[tokio::test]
    async fn failed_discoveries_keep_the_members_and_warn() {
        let pool = pool(Deployment::Sharded);
        let first = vec![found(&pool, "db1:27018", "shard01", NodeRole::Primary)];
        apply_discovery(&pool.inner, discovered(first));
        apply_discovery(&pool.inner, Err("listShards timed out".into()));
        assert_eq!(pool.members().len(), 1);
        // Reported with every snapshot while it holds.
        for _ in 0..2 {
            assert_eq!(
                pool.warnings(),
                ["could not discover the cluster members: listShards timed out"]
            );
        }
        assert!(pool.inner.state().last_discovery.is_some());
        // Until a discovery succeeds.
        let again = vec![found(&pool, "db1:27018", "shard01", NodeRole::Primary)];
        apply_discovery(&pool.inner, discovered(again));
        assert!(pool.warnings().is_empty());
    }

    #[tokio::test]
    async fn discovery_warnings_last_until_the_next_discovery() {
        let pool = pool(Deployment::Sharded);
        let partial = Ok(Discovered {
            members: vec![found(&pool, "db1:27018", "shard01", NodeRole::Primary)],
            warnings: vec!["could not find the config servers: timed out".into()],
        });
        apply_discovery(&pool.inner, partial);
        for _ in 0..2 {
            assert_eq!(
                pool.warnings(),
                ["could not find the config servers: timed out"]
            );
        }
        let complete = vec![found(&pool, "db1:27018", "shard01", NodeRole::Primary)];
        apply_discovery(&pool.inner, discovered(complete));
        assert!(pool.warnings().is_empty());
    }

    #[tokio::test]
    async fn member_shards_are_remembered() {
        let pool = pool(Deployment::Sharded);
        let first = vec![
            found(&pool, "db1:27018", "shard01", NodeRole::Primary),
            found(&pool, "cfg1:27019", CONFIG_SHARD, NodeRole::Primary),
        ];
        apply_discovery(&pool.inner, discovered(first));
        assert_eq!(pool.addresses_of("shard01"), ["db1:27018"]);
        // Member clients use the credential of their shard.
        assert!(pool.client_for("db1:27018", Users::Node).is_some());
        assert!(pool.client_for("db1:27018", Users::Cluster).is_none());
        assert!(pool.client_for("cfg1:27019", Users::Cluster).is_some());
        assert!(pool.client_for("cfg1:27019", Users::Node).is_none());
        // Gone from the cluster, the shard is still known.
        apply_discovery(&pool.inner, discovered(Vec::new()));
        assert!(pool.addresses_of("shard01").is_empty());
        assert_eq!(pool.shard_of("cfg1:27019").as_deref(), Some(CONFIG_SHARD));
        assert_eq!(pool.shard_of("db9:27018"), None);
    }

    #[tokio::test]
    async fn discoveries_after_shutdown_release_their_clients() {
        let pool = pool(Deployment::Sharded);
        let first = vec![found(&pool, "db1:27018", "shard01", NodeRole::Primary)];
        apply_discovery(&pool.inner, discovered(first));
        assert_eq!(pool.take_clients().len(), 1);
        let late = vec![found(&pool, "db2:27018", "shard01", NodeRole::Primary)];
        assert_eq!(apply_discovery(&pool.inner, discovered(late)).len(), 1);
        assert!(pool.members().is_empty());
        assert!(pool.ensure_discovery(true).is_none());
    }

    #[tokio::test]
    async fn failing_members_are_not_waited_for() {
        let pool = pool(Deployment::ReplicaSet { name: "rs0".into() });
        let member = Member {
            address: "db1:27017".into(),
            shard: "rs0".into(),
            set_name: Some("rs0".into()),
            role: NodeRole::Secondary,
            client: Err("cannot connect".into()),
        };
        pool.inner.state().members = vec![member.clone()];
        let members = vec![member];

        let first = pool.poll(&members, &[]).await;
        assert_eq!(first[0].outcome.result, Err("cannot connect".into()));
        let retry_at = pool.inner.state().failures["db1:27017"].retry_at;
        assert!(retry_at > Instant::now() + RETRY_INTERVAL - Duration::from_secs(1));

        // Reported right away, without a new poll.
        pool.inner
            .state()
            .failures
            .get_mut("db1:27017")
            .unwrap()
            .error = "remembered".into();
        let second = pool.poll(&members, &[]).await;
        assert_eq!(second[0].outcome.result, Err("remembered".into()));
        assert_eq!(second[0].outcome.role, NodeRole::Secondary);

        // When the retry is due, it runs in the background and the last
        // error is reported meanwhile.
        pool.inner
            .state()
            .failures
            .get_mut("db1:27017")
            .unwrap()
            .retry_at = Instant::now();
        let third = pool.poll(&members, &[]).await;
        assert_eq!(third[0].outcome.result, Err("remembered".into()));
        for _ in 0..100 {
            if pool.inner.state().failures["db1:27017"].error == "cannot connect" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the background retry did not record its outcome");
    }

    fn seed(address: &str, shard: &str) -> Seed {
        Seed {
            address: address.into(),
            shard: shard.into(),
            set_name: None,
        }
    }

    fn seed_in(address: &str, shard: &str, set_name: &str) -> Seed {
        Seed {
            set_name: Some(set_name.into()),
            ..seed(address, shard)
        }
    }

    fn probe(address: &str, shard: &str, hello: Option<Document>) -> Probe {
        Probe {
            seed: seed(address, shard),
            client: Err("no client in tests".into()),
            created: true,
            hello: hello.as_ref().map(Hello::parse),
        }
    }

    #[test]
    fn list_shards_seeds() {
        let response = doc! {
            "shards": [
                { "_id": "shard01", "host": "shard01/DB1:27018,db2:27018", "state": 1 },
                { "_id": "shard02", "host": "db3:27018" },
                { "_id": "broken" },
            ],
            "ok": 1,
        };
        assert_eq!(
            seeds_from_list_shards(&response),
            [
                seed_in("db1:27018", "shard01", "shard01"),
                seed_in("db2:27018", "shard01", "shard01"),
                seed("db3:27018", "shard02"),
            ]
        );
        assert_eq!(
            shard_hosts(&response, "shard01"),
            (
                vec!["db1:27018".to_owned(), "db2:27018".to_owned()],
                Some("shard01".to_owned())
            )
        );
        assert!(seeds_from_list_shards(&doc! { "ok": 1 }).is_empty());
        assert_eq!(shard_hosts(&response, "shard02").1, None);
        assert!(shard_hosts(&response, "shard09").0.is_empty());
    }

    #[test]
    fn config_server_strings() {
        let status = doc! { "sharding": { "configsvrConnectionString": "configRS/cfg1:27019" } };
        assert_eq!(config_servers_from(&status), Some("configRS/cfg1:27019"));
        let map = doc! { "map": { "shard01": "shard01/db1:27018", "config": "configRS/cfg1:27019,cfg2:27019" } };
        assert_eq!(
            config_servers_from(&map),
            Some("configRS/cfg1:27019,cfg2:27019")
        );
        assert_eq!(config_servers_from(&doc! { "process": "mongos" }), None);
        assert_eq!(
            seeds_from_hosts(CONFIG_SHARD, "configRS/cfg1:27019,[::1]:27019"),
            [
                seed_in("cfg1:27019", "config", "configRS"),
                seed_in("[::1]:27019", "config", "configRS")
            ]
        );
    }

    #[test]
    fn members_skip_arbiters_and_duplicates() {
        let probes = vec![
            probe(
                "db1:27018",
                "shard01",
                Some(doc! {
                    "setName": "shard01", "isWritablePrimary": true,
                    "hosts": ["db1:27018", "db2:27018"], "arbiters": ["db9:27018"],
                }),
            ),
            probe(
                "db2:27018",
                "shard01",
                Some(doc! { "setName": "shard01", "secondary": true }),
            ),
            // Down: role unknown, still a member.
            probe("db4:27018", "shard01", None),
            // Arbiter by its own hello.
            probe(
                "db8:27018",
                "shard01",
                Some(doc! { "setName": "shard01", "arbiterOnly": true }),
            ),
            // Arbiter as reported by the primary, down itself.
            probe("db9:27018", "shard01", None),
            probe(
                "cfg1:27019",
                "config",
                Some(doc! { "setName": "configRS", "isWritablePrimary": true }),
            ),
            probe("db1:27018", "shard01", None),
            probe(
                "db5:27018",
                "shard02",
                Some(doc! { "setName": "shard02", "isWritablePrimary": true }),
            ),
        ];
        let (members, dropped) = members_from_probes(probes);
        let summary: Vec<(&str, &str, NodeRole)> = members
            .iter()
            .map(|f| {
                (
                    f.member.address.as_str(),
                    f.member.shard.as_str(),
                    f.member.role,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("db1:27018", "shard01", NodeRole::Primary),
                ("db2:27018", "shard01", NodeRole::Secondary),
                ("db4:27018", "shard01", NodeRole::Unknown),
                ("db5:27018", "shard02", NodeRole::Primary),
                ("cfg1:27019", "config", NodeRole::Primary),
            ]
        );
        // No client was created in this test.
        assert!(dropped.is_empty());
    }

    #[test]
    fn unprobed_members_come_from_hello() {
        let probes = vec![
            probe(
                "db1:27018",
                "shard01",
                Some(doc! {
                    "setName": "shard01", "isWritablePrimary": true, "me": "db1:27018",
                    "hosts": ["db1:27018", "db2:27018"], "passives": ["DB3:27018"],
                    "arbiters": ["db9:27018"],
                }),
            ),
            // Listed under another name: hello.me tells.
            probe(
                "10.0.0.5:27018",
                "shard01",
                Some(doc! { "setName": "shard01", "secondary": true, "me": "db2:27018" }),
            ),
            probe("db7:27018", "shard02", None),
        ];
        // The reporter's replica set name binds the new member.
        assert_eq!(
            unprobed_members(&probes),
            [seed_in("db3:27018", "shard01", "shard01")]
        );
    }

    #[test]
    fn auth_failures_on_shard_members_get_a_hint() {
        let auth = CallError::Driver {
            what: "listing operations".into(),
            message: "authentication failed: SCRAM failure".into(),
            auth_failed: true,
        };
        let sharded = Hints {
            sharded: true,
            node_credential: false,
        };
        let hinted = member_error(&auth, "shard01", sharded);
        assert!(
            hinted.contains("use --node-username/--node-password"),
            "{hinted}"
        );
        let with_node_user = Hints {
            node_credential: true,
            ..sharded
        };
        let hinted = member_error(&auth, "shard01", with_node_user);
        assert!(
            hinted.contains("check --node-username/--node-password"),
            "{hinted}"
        );
        assert!(!member_error(&auth, CONFIG_SHARD, sharded).contains("--node-username"));
        let replica_set = Hints {
            sharded: false,
            node_credential: false,
        };
        assert!(!member_error(&auth, "rs0", replica_set).contains("--node-username"));
        let timeout = CallError::Timeout {
            what: "listing operations".into(),
            after: Duration::from_secs(3),
        };
        assert_eq!(
            member_error(&timeout, "shard01", sharded),
            "listing operations timed out after 3s"
        );
    }

    fn outcome(address: &str, shard: &str, role: NodeRole, ok: bool) -> PollOutcome {
        PollOutcome {
            address: address.into(),
            shard: shard.into(),
            role,
            result: if ok { Ok(2) } else { Err("down".into()) },
            latency: Duration::from_millis(7),
        }
    }

    fn member(address: &str, shard: &str, role: NodeRole) -> Member {
        Member {
            address: address.into(),
            shard: shard.into(),
            set_name: None,
            role,
            client: Err("no client in tests".into()),
        }
    }

    #[test]
    fn fallback_predicted_from_failing_members() {
        let members = vec![
            member("db1", "shard01", NodeRole::Primary),
            member("db2", "shard01", NodeRole::Secondary),
            member("db3", "shard02", NodeRole::Primary),
            member("db4", "shard02", NodeRole::Secondary),
            member("db5", "shard03", NodeRole::Unknown),
            member("cfg1", CONFIG_SHARD, NodeRole::Primary),
        ];
        let failing: HashSet<String> = ["db3", "db2", "cfg1"].map(String::from).into();
        // shard02's primary is failing; shard03 has no known primary.
        assert_eq!(
            predicted_fallback_shards(&members, &failing),
            ["shard02", "shard03"]
        );
        assert_eq!(
            predicted_fallback_shards(&members[..2], &HashSet::new()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn fallback_for_shards_without_a_polled_primary() {
        let outcomes = vec![
            outcome("db1", "shard01", NodeRole::Primary, true),
            outcome("db2", "shard01", NodeRole::Secondary, false),
            outcome("db3", "shard02", NodeRole::Primary, false),
            outcome("db4", "shard02", NodeRole::Secondary, true),
            outcome("db5", "shard03", NodeRole::Unknown, false),
            outcome("cfg1", CONFIG_SHARD, NodeRole::Primary, false),
        ];
        assert_eq!(shards_needing_fallback(&outcomes), ["shard02", "shard03"]);
        assert!(shards_needing_fallback(&outcomes[..2]).is_empty());
    }

    #[test]
    fn statuses_mark_fallback_members() {
        let outcomes = vec![
            outcome("db1", "shard01", NodeRole::Primary, true),
            outcome("db2", "shard01", NodeRole::Secondary, false),
            outcome("db3", "shard02", NodeRole::Primary, false),
            outcome("db4", "shard02", NodeRole::Secondary, true),
        ];
        let statuses = node_statuses(&outcomes, &["shard02".to_owned()]);
        let health: Vec<&NodeHealth> = statuses.iter().map(|s| &s.health).collect();
        assert_eq!(
            health,
            [
                &NodeHealth::Ok {
                    operations: 2,
                    latency: Duration::from_millis(7)
                },
                &NodeHealth::Failed {
                    error: "down".into()
                },
                &NodeHealth::Fallback {
                    error: "down".into()
                },
                &NodeHealth::Ok {
                    operations: 2,
                    latency: Duration::from_millis(7)
                },
            ]
        );
        assert_eq!(statuses[2].shard.as_deref(), Some("shard02"));
        assert_eq!(statuses[2].role, NodeRole::Primary);
    }
}
