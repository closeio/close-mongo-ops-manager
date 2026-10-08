//! MongoDB access: connecting, listing operations with `$currentOp` and
//! killing them.

mod call;
mod cluster;
mod kill;
mod options;
mod parse;
mod pipeline;
mod topology;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use futures::future::{OptionFuture, join_all};
use mongodb::Client;
use mongodb::bson::{Document, doc};
use mongodb::options::{Credential, ServerAddress};

use self::call::{CallError, admin_aggregate, admin_command, describe_error};
use self::cluster::{
    NodePool, node_statuses, predicted_fallback_shards, shard_hosts, shards_needing_fallback,
    shutdown_in_background, users_of,
};
use self::kill::{
    KillPlan, KillTiming, ServerBackend, Unreachable, execute, execute_through_mongos, plan_kill,
};
use self::options::{DirectClients, Users, main_options};
use self::parse::{OpContext, merge_operations, parse_operations};
use self::pipeline::{LISTING_BATCH_SIZE, ListingSpec, listing_pipeline};
use self::topology::{Hello, format_address, host_info_name, normalize_address};
use crate::error::MongoOpsError;
use crate::model::{
    Deployment, FetchQuery, KillOutcome, KillRequest, MAX_OPERATIONS, OpId, OpSource, Operation,
    ServerInfo, Snapshot,
};

/// Application name sent to the server; also used to hide our own operations.
pub const APP_NAME: &str = "close-mongo-ops-manager";

/// Default timeout for every server round trip.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Warning while no cluster member is known in per-node mode.
const NO_MEMBERS: &str =
    "no cluster member could be polled directly: listing operations through the main connection";

/// What to connect to.
#[derive(Debug, Clone)]
pub enum ConnectTarget {
    /// A connection string (`mongodb://` or `mongodb+srv://`).
    Uri(String),
    /// Seed list built from `--host`/`--port`.
    Hosts(Vec<ServerAddress>),
}

/// Connection settings.
#[derive(Debug, Clone)]
pub struct ConnectConfig {
    pub target: ConnectTarget,
    /// Credential from `--username`/`--password`. Used with
    /// [`ConnectTarget::Hosts`], and with a URI that has no credentials.
    pub credential: Option<Credential>,
    pub load_balanced: bool,
    /// Only list operations whose namespace starts with this (case-insensitive).
    pub namespace: String,
    pub hide_system_ops: bool,
    /// Discover every cluster member and poll each one directly.
    pub all_nodes: bool,
    /// Credential for direct connections to cluster members; defaults to the
    /// main credential.
    pub node_credential: Option<Credential>,
    /// Timeout for every server round trip.
    pub timeout: Duration,
}

/// Connection to a deployment.
///
/// `fetch` and `kill` may run concurrently (through an `Arc`), and a `fetch`
/// may be dropped at any await point: shared state is only changed in
/// synchronous steps.
pub struct MongoManager {
    client: Client,
    info: ServerInfo,
    namespace: String,
    hide_system_ops: bool,
    timeout: Duration,
    direct: DirectClients,
    /// Addresses of the servers behind the main connection (seeds, replica
    /// set members), to find the server that listed an operation.
    servers: Vec<String>,
    /// Cluster members, in per-node mode (`--all-nodes`).
    nodes: Option<NodePool>,
    /// Direct clients created for kills.
    adhoc: Mutex<HashMap<AdhocKey, Client>>,
    /// The last snapshot was truncated (to log changes only).
    truncated: AtomicBool,
    /// [`NO_MEMBERS`] holds (to log it once).
    no_members: AtomicBool,
    kill_timing: KillTiming,
}

// `MongoManager` is shared between tokio tasks.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MongoManager>();
};

/// The futures of the public methods can be spawned on tokio tasks.
const _: fn(&MongoManager, &FetchQuery, &KillRequest) = assert_send_futures;

fn assert_send_futures(manager: &MongoManager, query: &FetchQuery, request: &KillRequest) {
    fn send<T: Send>(_: T) {}
    send(manager.fetch(query));
    send(manager.kill(request));
    send(manager.shutdown());
}

/// Address, credential and required replica set name of an ad-hoc client.
type AdhocKey = (String, Users, Option<String>);

/// Operations listed by one query.
struct Listing {
    ops: Vec<Operation>,
    /// More than [`MAX_OPERATIONS`] matched.
    truncated: bool,
}

/// What a per-node refresh gathers.
#[derive(Default)]
struct Gathered {
    lists: Vec<Vec<Operation>>,
    truncated: bool,
    warnings: Vec<String>,
    /// Shards whose operations were listed through mongos.
    fallback_shards: Vec<String>,
}

impl Gathered {
    fn add(&mut self, listing: Listing) {
        self.truncated |= listing.truncated;
        self.lists.push(listing.ops);
    }

    /// Adds the operations of `shards` from a fallback listing through
    /// mongos (which may cover more shards).
    fn add_fallback(&mut self, result: Result<Listing, CallError>, shards: &[String]) {
        if shards.is_empty() {
            return;
        }
        match result {
            Ok(listing) => {
                let in_shards =
                    |op: &Operation| op.shard.as_ref().is_some_and(|s| shards.contains(s));
                self.add(Listing {
                    ops: listing.ops.into_iter().filter(in_shards).collect(),
                    truncated: listing.truncated,
                });
                self.fallback_shards.extend_from_slice(shards);
            }
            Err(error) => self.warnings.push(format!(
                "could not list the operations of {} through mongos either: {error}",
                shards.join(", ")
            )),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// `{shard: {$in: shards}}`.
fn shard_filter(shards: &[String]) -> Document {
    doc! { "shard": { "$in": shards } }
}

impl MongoManager {
    /// Connects and identifies the deployment.
    pub async fn connect(config: ConnectConfig) -> Result<Self, MongoOpsError> {
        let timeout = config.timeout;
        let main = main_options(&config).await?;
        let client = Client::with_options(main.options.clone()).map_err(|e| {
            MongoOpsError::Connection(format!("{}: {}", main.target, describe_error(&e)))
        })?;
        let (hello, version) = match identify(&client, timeout).await {
            Ok(identity) => identity,
            Err(error) => {
                shutdown_in_background(vec![client], timeout);
                return Err(error);
            }
        };

        let deployment = hello.deployment();
        if config.all_nodes && deployment == Deployment::Standalone {
            log::info!("Standalone server: --all-nodes has no effect");
        }
        let info = ServerInfo {
            target: main.target.clone(),
            version,
            all_nodes: config.all_nodes && deployment != Deployment::Standalone,
            load_balanced: main.load_balanced || hello.service_id,
            deployment,
        };
        log::info!("Connected to {} ({})", info.target, info.describe());

        let mut servers: Vec<String> = main.options.hosts.iter().map(format_address).collect();
        for member in hello.data_members().filter_map(normalize_address) {
            if !servers.contains(&member) {
                servers.push(member);
            }
        }

        let direct = DirectClients::new(&main, config.node_credential);
        let nodes = info.all_nodes.then(|| {
            NodePool::new(
                client.clone(),
                info.deployment.clone(),
                direct.clone(),
                timeout,
            )
        });
        let no_members = AtomicBool::new(false);
        if let Some(pool) = &nodes {
            pool.discover_now().await;
            match pool.members().len() {
                0 => {
                    log::warn!("{NO_MEMBERS}");
                    no_members.store(true, Ordering::Relaxed);
                }
                n => log::info!("Polling {n} cluster members directly"),
            }
        }

        Ok(Self {
            client,
            info,
            namespace: config.namespace.trim().to_owned(),
            hide_system_ops: config.hide_system_ops,
            timeout,
            direct,
            servers,
            nodes,
            adhoc: Mutex::new(HashMap::new()),
            truncated: AtomicBool::new(false),
            no_members,
            kill_timing: KillTiming::default(),
        })
    }

    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// Lists the current operations.
    ///
    /// The warnings of a snapshot describe the problems that hold at the
    /// time: a lasting problem is reported with every snapshot.
    pub async fn fetch(&self, query: &FetchQuery) -> Result<Snapshot, MongoOpsError> {
        let snapshot = match &self.nodes {
            Some(pool) => self.fetch_all_nodes(pool, query).await?,
            None => self.fetch_main(query).await?,
        };
        if snapshot.truncated && !self.truncated.swap(true, Ordering::Relaxed) {
            log::warn!(
                "Operation list truncated to the {MAX_OPERATIONS} longest running operations"
            );
        } else if !snapshot.truncated && self.truncated.swap(false, Ordering::Relaxed) {
            log::info!("Operation list no longer truncated");
        }
        Ok(snapshot)
    }

    /// Kills one operation on the server that reported it.
    pub async fn kill(&self, request: &KillRequest) -> KillOutcome {
        log::info!(
            "Killing operation {} ({}): {}",
            request.opid,
            source_label(&request.source),
            request.description
        );
        let outcome = match plan_kill(request, self.info.is_sharded()) {
            Ok(plan) => self.kill_with(request, plan).await,
            Err(reason) => KillOutcome::Refused(reason),
        };
        if outcome.is_success() {
            log::info!("Operation {}: {outcome}", request.opid);
        } else {
            log::warn!("Operation {}: {outcome}", request.opid);
        }
        outcome
    }

    /// Closes every connection.
    pub async fn shutdown(&self) {
        let mut clients = vec![self.client.clone()];
        if let Some(pool) = &self.nodes {
            clients.extend(pool.take_clients());
        }
        clients.extend(lock(&self.adhoc).drain().map(|(_, client)| client));
        let timeout = self.timeout;
        join_all(clients.into_iter().map(|client| async move {
            let _ = tokio::time::timeout(timeout, client.shutdown().immediate(true)).await;
        }))
        .await;
        log::info!("Closed MongoDB connections");
    }

    fn pipeline(
        &self,
        query: &FetchQuery,
        local_ops: bool,
        extra_match: Option<Document>,
    ) -> Vec<Document> {
        listing_pipeline(&ListingSpec {
            filters: &query.filters,
            namespace: &self.namespace,
            hide_system_ops: self.hide_system_ops,
            local_ops,
            extra_match,
        })
    }

    async fn list(
        &self,
        client: &Client,
        what: &str,
        pipeline: Vec<Document>,
        ctx: &OpContext<'_>,
    ) -> Result<Listing, CallError> {
        let docs =
            admin_aggregate(client, what, pipeline, LISTING_BATCH_SIZE, self.timeout).await?;
        Ok(Listing {
            truncated: docs.len() > MAX_OPERATIONS,
            ops: parse_operations(docs, ctx),
        })
    }

    /// Operations listed through the main connection (every shard primary
    /// through mongos), optionally restricted by `extra_match`.
    async fn list_main(
        &self,
        query: &FetchQuery,
        extra_match: Option<Document>,
    ) -> Result<Listing, CallError> {
        let ctx = OpContext {
            source: &OpSource::Main,
            shard: None,
            node_role: None,
            default_host: None,
        };
        let pipeline = self.pipeline(query, false, extra_match);
        self.list(&self.client, "listing operations", pipeline, &ctx)
            .await
    }

    /// The connected mongos' own operations.
    async fn list_mongos_local(&self, query: &FetchQuery) -> Result<Listing, CallError> {
        let ctx = OpContext {
            source: &OpSource::MongosLocal,
            shard: None,
            node_role: None,
            default_host: None,
        };
        let pipeline = self.pipeline(query, true, None);
        self.list(
            &self.client,
            "listing the mongos' own operations",
            pipeline,
            &ctx,
        )
        .await
    }

    fn wants_mongos_local(&self, query: &FetchQuery) -> bool {
        query.include_mongos_local && self.info.is_sharded()
    }

    /// Default mode: everything through the main connection.
    async fn fetch_main(&self, query: &FetchQuery) -> Result<Snapshot, MongoOpsError> {
        let local = OptionFuture::from(
            self.wants_mongos_local(query)
                .then(|| self.list_mongos_local(query)),
        );
        let (main, local) = tokio::join!(self.list_main(query, None), local);
        let main = main.map_err(CallError::into_operation_error)?;
        let mut truncated = main.truncated;
        let mut lists = vec![main.ops];
        let mut warnings = Vec::new();
        add_mongos_local(local, &mut lists, &mut truncated, &mut warnings);
        let (operations, truncated) = merge_operations(lists, truncated);
        Ok(Snapshot {
            operations,
            truncated,
            nodes: Vec::new(),
            warnings,
        })
    }

    /// Per-node mode: every member through its own connection.
    async fn fetch_all_nodes(
        &self,
        pool: &NodePool,
        query: &FetchQuery,
    ) -> Result<Snapshot, MongoOpsError> {
        let members = pool.members_to_poll();
        let mut warnings = pool.warnings();
        if members.is_empty() {
            // Discovery is retried in the background meanwhile.
            if !self.no_members.swap(true, Ordering::Relaxed) {
                log::warn!("{NO_MEMBERS}");
            }
            let mut snapshot = self.fetch_main(query).await?;
            warnings.push(NO_MEMBERS.to_owned());
            warnings.append(&mut snapshot.warnings);
            snapshot.warnings = warnings;
            return Ok(snapshot);
        }
        self.no_members.store(false, Ordering::Relaxed);

        // Shards whose primary is known to fail are listed through mongos
        // along with the polls.
        let sharded = self.info.is_sharded();
        let predicted = if sharded {
            predicted_fallback_shards(&members, &pool.failing())
        } else {
            Vec::new()
        };
        let early_fallback = OptionFuture::from(
            (!predicted.is_empty()).then(|| self.list_main(query, Some(shard_filter(&predicted)))),
        );
        let local = OptionFuture::from(
            self.wants_mongos_local(query)
                .then(|| self.list_mongos_local(query)),
        );
        let pipeline = self.pipeline(query, false, None);
        let (polls, local, early_fallback) =
            tokio::join!(pool.poll(&members, &pipeline), local, early_fallback);

        let mut gathered = Gathered::default();
        let mut outcomes = Vec::with_capacity(polls.len());
        for poll in polls {
            gathered.truncated |= poll.truncated;
            gathered.lists.push(poll.ops);
            outcomes.push(poll.outcome);
        }

        if sharded {
            let needed = shards_needing_fallback(&outcomes);
            let (covered, missing): (Vec<String>, Vec<String>) =
                needed.into_iter().partition(|s| predicted.contains(s));
            if let Some(result) = early_fallback {
                gathered.add_fallback(result, &covered);
            }
            if !missing.is_empty() {
                let result = self.list_main(query, Some(shard_filter(&missing))).await;
                gathered.add_fallback(result, &missing);
            }
            if !gathered.fallback_shards.is_empty() {
                log::debug!(
                    "Listed the operations of {} through mongos",
                    gathered.fallback_shards.join(", ")
                );
            }
        }

        if gathered.fallback_shards.is_empty()
            && let Some(first) = outcomes.iter().find_map(|o| o.result.as_ref().err())
            && outcomes.iter().all(|o| o.result.is_err())
        {
            return Err(MongoOpsError::Operation(format!(
                "could not list operations on any cluster member: {first}"
            )));
        }

        let Gathered {
            mut lists,
            mut truncated,
            warnings: mut fetch_warnings,
            fallback_shards,
        } = gathered;
        add_mongos_local(local, &mut lists, &mut truncated, &mut fetch_warnings);
        warnings.append(&mut fetch_warnings);
        let (operations, truncated) = merge_operations(lists, truncated);
        Ok(Snapshot {
            operations,
            truncated,
            nodes: node_statuses(&outcomes, &fallback_shards),
            warnings,
        })
    }

    async fn kill_with(&self, request: &KillRequest, plan: KillPlan) -> KillOutcome {
        let timing = self.kill_timing;
        match plan {
            KillPlan::Mongos { shard, number } => {
                let mongos = self.backend(self.client.clone(), request.opid.clone(), false);
                let member = self.shard_member_backend(request, &shard, number);
                execute_through_mongos(&mongos, member, request, timing).await
            }
            KillPlan::Mongod { host } => {
                let explanation = format!("cannot reach {host}, the server that listed it");
                let set_name = self.main_set_name();
                let address = self
                    .reachable_address(
                        &host,
                        &self.main_servers(),
                        Users::Cluster,
                        set_name.as_deref(),
                    )
                    .await;
                match self.direct_client(&address, Users::Cluster, set_name.as_deref()) {
                    Ok(client) => {
                        let backend = self.backend(client, request.opid.clone(), false);
                        let unreachable = Unreachable::FailWith(&explanation);
                        execute(&backend, request, timing, unreachable).await
                    }
                    Err(error) => KillOutcome::Failed(format!("{explanation}: {error}")),
                }
            }
            KillPlan::MongosLocal { host } => {
                // Only by its own name: the address of the main connection
                // may lead to another mongos.
                let explanation =
                    format!("the mongos that listed it ({host}) cannot be reached directly");
                match self.direct_client(&host, Users::Cluster, None) {
                    Ok(client) => {
                        let backend = self.backend(client, request.opid.clone(), true);
                        let unreachable = Unreachable::Refuse(&explanation);
                        execute(&backend, request, timing, unreachable).await
                    }
                    Err(error) => KillOutcome::Refused(format!("{explanation}: {error}")),
                }
            }
            KillPlan::Member { address } => {
                let users = self
                    .nodes
                    .as_ref()
                    .and_then(|p| p.shard_of(&address))
                    .map_or(Users::Node, |shard| users_of(&shard));
                let set_name = self.nodes.as_ref().and_then(|p| p.set_name_of(&address));
                match self.direct_client(&address, users, set_name.as_deref()) {
                    Ok(client) => {
                        let backend = self.backend(client, request.opid.clone(), false);
                        execute(&backend, request, timing, Unreachable::Fail).await
                    }
                    Err(error) => KillOutcome::Failed(error),
                }
            }
        }
    }

    fn backend(&self, client: Client, opid: OpId, local_ops: bool) -> ServerBackend {
        ServerBackend {
            client,
            opid,
            local_ops,
            timeout: self.timeout,
        }
    }

    /// A connection to the shard member that listed a [`KillPlan::Mongos`]
    /// operation, to confirm it is gone.
    async fn shard_member_backend(
        &self,
        request: &KillRequest,
        shard: &str,
        number: i64,
    ) -> Result<ServerBackend, String> {
        let host = request
            .host
            .as_deref()
            .ok_or("the server that listed it is unknown")?;
        let users = users_of(shard);
        let (candidates, set_name) = self.shard_servers(shard).await;
        let set_name = set_name.as_deref();
        let address = self
            .reachable_address(host, &candidates, users, set_name)
            .await;
        let client = self.direct_client(&address, users, set_name)?;
        Ok(self.backend(client, OpId::Num(number), false))
    }

    /// The members of `shard` and its replica set name: from the cluster
    /// members, or `listShards`.
    async fn shard_servers(&self, shard: &str) -> (Vec<String>, Option<String>) {
        if let Some(pool) = &self.nodes {
            let members = pool.addresses_of(shard);
            if !members.is_empty() {
                return (members, pool.shard_set_name(shard));
            }
        }
        match admin_command(
            &self.client,
            "listShards",
            doc! { "listShards": 1 },
            self.timeout,
        )
        .await
        {
            Ok(response) => shard_hosts(&response, shard),
            Err(error) => {
                log::debug!("Could not list the members of {shard}: {error}");
                (Vec::new(), None)
            }
        }
    }

    /// The servers behind the main connection (replica set or standalone).
    fn main_servers(&self) -> Vec<String> {
        let mut servers = self.servers.clone();
        if let Some(pool) = &self.nodes {
            for member in pool.members() {
                if !servers.contains(&member.address) {
                    servers.push(member.address);
                }
            }
        }
        servers
    }

    /// The replica set name of the main connection's servers, if any.
    fn main_set_name(&self) -> Option<String> {
        match &self.info.deployment {
            Deployment::ReplicaSet { name } => Some(name.clone()),
            _ => None,
        }
    }

    /// The address to connect to for the server that calls itself `host` in
    /// `$currentOp`: its own host name, which may not resolve here (e.g. a
    /// container's), so each of this kill's `candidates` is asked for the
    /// name it gives itself (`hostInfo`). Names and addresses change with the
    /// topology and may collide, so nothing is remembered between kills and
    /// only a single match counts. Falls back to `host` itself.
    async fn reachable_address(
        &self,
        host: &str,
        candidates: &[String],
        users: Users,
        set_name: Option<&str>,
    ) -> String {
        let Some(host) = normalize_address(host) else {
            return host.to_owned();
        };
        if candidates.contains(&host) {
            return host;
        }
        let wanted = host.as_str();
        let matches: Vec<String> = join_all(candidates.iter().map(|address| async move {
            let client = self.direct_client(address, users, set_name).ok()?;
            let info = admin_command(&client, "hostInfo", doc! { "hostInfo": 1 }, self.timeout)
                .await
                .map_err(|e| log::debug!("Could not identify {address}: {e}"))
                .ok()?;
            (host_info_name(&info)? == wanted).then(|| address.clone())
        }))
        .await
        .into_iter()
        .flatten()
        .collect();
        match matches.as_slice() {
            [address] => address.clone(),
            // Unknown or ambiguous: only the server's own name is left.
            _ => host,
        }
    }

    /// A direct client for `address`: the cluster member's, or a new one.
    /// With `set_name`, the client refuses a server of another replica set.
    fn direct_client(
        &self,
        address: &str,
        users: Users,
        set_name: Option<&str>,
    ) -> Result<Client, String> {
        let address =
            normalize_address(address).ok_or_else(|| format!("invalid address {address}"))?;
        if let Some(client) = self
            .nodes
            .as_ref()
            .and_then(|p| p.client_for(&address, users))
        {
            return Ok(client);
        }
        let mut adhoc = lock(&self.adhoc);
        let key = (address, users, set_name.map(str::to_owned));
        if let Some(client) = adhoc.get(&key) {
            return Ok(client.clone());
        }
        let client = self.direct.client(&key.0, users, set_name)?;
        adhoc.insert(key, client.clone());
        Ok(client)
    }
}

/// Checks the connection and identifies the server: its `hello` and version.
async fn identify(client: &Client, timeout: Duration) -> Result<(Hello, String), MongoOpsError> {
    admin_command(client, "ping", doc! { "ping": 1 }, timeout)
        .await
        .map_err(CallError::into_connection_error)?;
    let hello = match admin_command(client, "hello", doc! { "hello": 1 }, timeout).await {
        Ok(hello) => hello,
        // Servers before 4.4.2 only know the legacy name.
        Err(error @ CallError::Driver { .. }) => {
            admin_command(client, "isMaster", doc! { "isMaster": 1 }, timeout)
                .await
                .map_err(|_| error.into_connection_error())?
        }
        Err(error) => return Err(error.into_connection_error()),
    };
    let build_info = admin_command(client, "buildInfo", doc! { "buildInfo": 1 }, timeout)
        .await
        .map_err(CallError::into_connection_error)?;
    let version = build_info
        .get_str("version")
        .unwrap_or("unknown version")
        .to_owned();
    Ok((Hello::parse(&hello), version))
}

/// Adds the mongos' own operations to a listing, or a warning.
fn add_mongos_local(
    local: Option<Result<Listing, CallError>>,
    lists: &mut Vec<Vec<Operation>>,
    truncated: &mut bool,
    warnings: &mut Vec<String>,
) {
    match local {
        Some(Ok(listing)) => {
            *truncated |= listing.truncated;
            lists.push(listing.ops);
        }
        Some(Err(error)) => {
            log::warn!("Could not list the mongos' own operations: {error}");
            warnings.push(format!(
                "could not list the mongos' own operations: {error}"
            ));
        }
        None => {}
    }
}

fn source_label(source: &OpSource) -> String {
    match source {
        OpSource::Main => "main connection".to_owned(),
        OpSource::MongosLocal => "mongos".to_owned(),
        OpSource::Node { address } => address.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OpKey;
    use crate::testutil::sample_operation;

    fn shard_op(shard: &str, opid: i64) -> Operation {
        let mut op = sample_operation(OpId::Str(format!("{shard}:{opid}")));
        op.key = OpKey(op.opid.to_string());
        op.shard = Some(shard.into());
        op
    }

    fn listing(ops: Vec<Operation>) -> Result<Listing, CallError> {
        Ok(Listing {
            ops,
            truncated: true,
        })
    }

    #[test]
    fn fallback_listings_only_keep_the_needed_shards() {
        let mut gathered = Gathered::default();
        let ops = vec![shard_op("shard01", 1), shard_op("shard02", 2)];
        gathered.add_fallback(listing(ops), &["shard02".to_owned()]);
        let kept: Vec<&str> = gathered.lists[0]
            .iter()
            .map(|o| o.shard.as_deref().unwrap())
            .collect();
        assert_eq!(kept, ["shard02"]);
        assert!(gathered.truncated);
        assert_eq!(gathered.fallback_shards, ["shard02"]);
        assert!(gathered.warnings.is_empty());
    }

    #[test]
    fn unneeded_fallback_listings_are_ignored() {
        let mut gathered = Gathered::default();
        gathered.add_fallback(listing(vec![shard_op("shard01", 1)]), &[]);
        let error = CallError::Timeout {
            what: "listing operations".into(),
            after: Duration::from_secs(3),
        };
        gathered.add_fallback(Err(error.clone()), &[]);
        assert!(gathered.lists.is_empty());
        assert!(gathered.warnings.is_empty());
        assert!(!gathered.truncated);

        gathered.add_fallback(Err(error), &["shard01".to_owned(), "shard02".to_owned()]);
        assert_eq!(
            gathered.warnings,
            [
                "could not list the operations of shard01, shard02 through mongos either: \
              listing operations timed out after 3s"
            ]
        );
        assert!(gathered.fallback_shards.is_empty());
    }

    #[test]
    fn shard_filters() {
        assert_eq!(
            shard_filter(&["shard01".to_owned()]),
            doc! { "shard": { "$in": ["shard01"] } }
        );
    }
}
