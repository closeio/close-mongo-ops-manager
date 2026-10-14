//! Integration tests of the MongoDB layer against real servers.
//!
//! Start the test cluster and export the variables it prints:
//!
//! ```text
//! eval "$(tests/docker/cluster.sh up)"
//! cargo test --test mongo_integration
//! ```
//!
//! The tests share the cluster (and kill each other's leftovers), so they run
//! one at a time.
//!
//! Each test returns early (and passes) when the variables it needs are not
//! set:
//!
//! * `CMOM_IT_MONGOS_URI`: a mongos of a sharded cluster with shards
//!   `shard01` (primary, secondary, arbiter) and `shard02` (primary, passive
//!   secondary) and the sharded collection `cmomit.load`.
//! * `CMOM_IT_MONGOS_MULTI_URI`: two mongos of the same cluster.
//! * `CMOM_IT_RS_URI`: the replica set `shard01`.
//! * `CMOM_IT_STANDALONE_URI`: a standalone mongod with `cmomit.load`.
//! * `CMOM_IT_PLAIN_URI`: a standalone mongod whose own host name (the host
//!   of its operations) does not resolve here, like in a container.
//! * `CMOM_IT_CONTAINER`: the container running every process, to stop and
//!   restart a member.

use std::process::Command;
use std::time::{Duration, Instant};

use close_mongo_ops_manager::error::MongoOpsError;
use close_mongo_ops_manager::model::{
    Deployment, FetchQuery, Filters, KillOutcome, KillRequest, NodeHealth, NodeRole, OpId,
    OpSource, Operation, Snapshot,
};
use close_mongo_ops_manager::mongo::{APP_NAME, ConnectConfig, ConnectTarget, MongoManager};
use futures::TryStreamExt;
use futures::future::join_all;
use mongodb::Client;
use mongodb::bson::{Document, doc};
use mongodb::error::ErrorKind;
use mongodb::options::{ClientOptions, Credential};
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(3);
/// How long to wait for an operation to show up.
const APPEAR_WITHIN: Duration = Duration::from_secs(20);

/// Serializes the tests: they share one cluster.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The value of an environment variable, or skip the test.
macro_rules! require {
    ($name:literal) => {
        match env_var($name) {
            Some(value) => value,
            None => {
                eprintln!(
                    "skipped: {} is not set (see tests/docker/cluster.sh)",
                    $name
                );
                return;
            }
        }
    };
}

fn config(uri: &str) -> ConnectConfig {
    ConnectConfig {
        target: ConnectTarget::Uri(uri.to_owned()),
        credential: None,
        load_balanced: false,
        namespace: String::new(),
        hide_system_ops: true,
        all_nodes: false,
        node_credential: None,
        timeout: TIMEOUT,
    }
}

fn all_nodes(uri: &str) -> ConnectConfig {
    ConnectConfig {
        all_nodes: true,
        ..config(uri)
    }
}

async fn connect(config: ConnectConfig) -> MongoManager {
    let target = format!("{:?}", config.target);
    match MongoManager::connect(config).await {
        Ok(manager) => manager,
        Err(error) => panic!("could not connect to {target}: {error}"),
    }
}

fn with_mongos_local() -> FetchQuery {
    FetchQuery {
        include_mongos_local: true,
        ..FetchQuery::default()
    }
}

fn with_filters(filters: Filters) -> FetchQuery {
    FetchQuery {
        filters,
        ..FetchQuery::default()
    }
}

/// A slow query in the background: a collection scan sleeping 100 ms per
/// document, so it runs for tens of seconds.
struct Load {
    app: String,
    task: JoinHandle<mongodb::error::Result<usize>>,
}

/// Starts a slow query on `cmomit.load` through `uri`, with application name
/// `app`. Through mongos it runs on both shards.
async fn start_load(uri: &str, app: &str) -> Load {
    start_load_matching(uri, app, Document::new()).await
}

/// Like [`start_load`], but through mongos it only runs on `shard01` (which
/// holds `k < 500`). Killing one shard's part of a query does not end it
/// before the other shards answered, so kill tests use this.
async fn start_shard01_load(uri: &str, app: &str) -> Load {
    start_load_matching(uri, app, doc! { "k": { "$lt": 500 } }).await
}

async fn start_load_matching(uri: &str, app: &str, mut filter: Document) -> Load {
    let mut options = ClientOptions::parse(uri).await.expect("load URI");
    options.app_name = Some(app.to_owned());
    let client = Client::with_options(options).expect("load client");
    // Commands select the primary: once it is known, `primaryPreferred`
    // reads do not go to a secondary found first.
    client
        .database("admin")
        .run_command(doc! { "ping": 1 })
        .await
        .expect("load ping");
    let collection = client.database("cmomit").collection::<Document>("load");
    filter.insert("$where", "sleep(100) || false");
    let task = tokio::spawn(async move {
        let cursor = collection
            .find(filter)
            .max_time(Duration::from_secs(120))
            .await?;
        let docs: Vec<Document> = cursor.try_collect().await?;
        Ok(docs.len())
    });
    Load {
        app: app.to_owned(),
        task,
    }
}

impl Load {
    fn is_mine(&self, op: &Operation) -> bool {
        op.app_name.as_deref() == Some(self.app.as_str())
    }

    /// Waits for the query to fail because it was killed.
    async fn expect_interrupted(self) {
        let result = tokio::time::timeout(Duration::from_secs(15), self.task)
            .await
            .expect("the killed query did not end")
            .expect("load task");
        match result {
            Ok(n) => panic!("the killed query succeeded ({n} documents)"),
            Err(error) => assert!(is_interrupted(&error), "unexpected error: {error}"),
        }
        kill_leftovers(&self.app).await;
    }

    /// Stops the query and kills whatever is left of it on the servers.
    async fn stop(self) {
        self.task.abort();
        kill_leftovers(&self.app).await;
    }
}

fn is_interrupted(error: &mongodb::error::Error) -> bool {
    const INTERRUPTED: i32 = 11601;
    matches!(error.kind.as_ref(), ErrorKind::Command(e) if e.code == INTERRUPTED)
        || error.to_string().to_lowercase().contains("interrupted")
}

/// Best effort: kills every operation of `app` still running anywhere.
async fn kill_leftovers(app: &str) {
    let mut configs = Vec::new();
    if let Some(uri) = env_var("CMOM_IT_MONGOS_URI") {
        configs.push(all_nodes(&uri));
    }
    if let Some(uri) = env_var("CMOM_IT_MONGOS_MULTI_URI") {
        // The second mongos' own operations.
        if let Some(last) = last_host_uri(&uri).await {
            configs.push(config(&last));
        }
    }
    for uri in [
        env_var("CMOM_IT_RS_URI"),
        env_var("CMOM_IT_STANDALONE_URI"),
        env_var("CMOM_IT_PLAIN_URI"),
    ]
    .into_iter()
    .flatten()
    {
        configs.push(config(&uri));
    }
    for config in configs {
        let Ok(manager) = MongoManager::connect(config).await else {
            continue;
        };
        if let Ok(snapshot) = manager.fetch(&with_mongos_local()).await {
            let requests: Vec<KillRequest> = snapshot
                .operations
                .iter()
                .filter(|o| o.app_name.as_deref() == Some(app))
                .map(KillRequest::from_operation)
                .collect();
            join_all(requests.iter().map(|r| manager.kill(r))).await;
        }
        manager.shutdown().await;
    }
}

/// `mongodb://<last host>/` of a multi-host URI.
async fn last_host_uri(uri: &str) -> Option<String> {
    let options = ClientOptions::parse(uri).await.ok()?;
    let last = options.hosts.last()?;
    Some(format!("mongodb://{last}/"))
}

/// Fetches until an operation matching `found` is listed.
async fn wait_for(
    manager: &MongoManager,
    query: &FetchQuery,
    what: &str,
    found: impl Fn(&Operation) -> bool,
) -> (Operation, Snapshot) {
    let deadline = Instant::now() + APPEAR_WITHIN;
    loop {
        let snapshot = manager.fetch(query).await.expect("fetch");
        if let Some(op) = snapshot.operations.iter().find(|o| found(o)) {
            return (op.clone(), snapshot);
        }
        assert!(Instant::now() < deadline, "{what} did not show up");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Fetches with `query` and returns the operations of `load`.
async fn load_ops(manager: &MongoManager, query: &FetchQuery, load: &Load) -> Vec<Operation> {
    let snapshot = manager.fetch(query).await.expect("fetch");
    snapshot
        .operations
        .into_iter()
        .filter(|o| load.is_mine(o))
        .collect()
}

fn shard_of(op: &Operation) -> Option<&str> {
    op.shard.as_deref()
}

fn node_summary(snapshot: &Snapshot) -> Vec<(String, String, NodeRole, bool)> {
    snapshot
        .nodes
        .iter()
        .map(|n| {
            (
                n.address.clone(),
                n.shard.clone().unwrap_or_default(),
                n.role,
                n.health.is_ok(),
            )
        })
        .collect()
}

fn node_health<'a>(snapshot: &'a Snapshot, address: &str) -> &'a NodeHealth {
    &snapshot
        .nodes
        .iter()
        .find(|n| n.address == address)
        .unwrap_or_else(|| panic!("no status for {address}: {:?}", snapshot.nodes))
        .health
}

fn sharded_cluster_nodes() -> Vec<(String, String, NodeRole, bool)> {
    [
        ("localhost:37021", "shard01", NodeRole::Primary),
        ("localhost:37022", "shard01", NodeRole::Secondary),
        ("localhost:37024", "shard02", NodeRole::Primary),
        ("localhost:37025", "shard02", NodeRole::Secondary),
        ("localhost:37019", "config", NodeRole::Primary),
    ]
    .into_iter()
    .map(|(a, s, r)| (a.to_owned(), s.to_owned(), r, true))
    .collect()
}

#[tokio::test]
async fn detects_mongos() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(config(&uri)).await;
    let info = manager.info().clone();
    assert_eq!(info.deployment, Deployment::Sharded);
    assert_eq!(info.target, "localhost:37017");
    assert!(
        info.version.starts_with(char::is_numeric),
        "{}",
        info.version
    );
    assert!(!info.load_balanced);
    assert!(!info.all_nodes);
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    assert!(snapshot.nodes.is_empty());
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    manager.shutdown().await;
}

#[tokio::test]
async fn detects_a_replica_set() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_RS_URI");
    let manager = connect(config(&uri)).await;
    let info = manager.info();
    assert_eq!(
        info.deployment,
        Deployment::ReplicaSet {
            name: "shard01".into()
        }
    );
    assert_eq!(info.target, "localhost:37021 +1");
    manager.fetch(&FetchQuery::default()).await.unwrap();
    manager.shutdown().await;
}

#[tokio::test]
async fn detects_a_standalone_server() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_STANDALONE_URI");
    // --all-nodes has no effect on a standalone server.
    let manager = connect(all_nodes(&uri)).await;
    let info = manager.info();
    assert_eq!(info.deployment, Deployment::Standalone);
    assert_eq!(info.target, "localhost:37030");
    assert!(!info.all_nodes);
    let snapshot = manager.fetch(&with_mongos_local()).await.unwrap();
    assert!(snapshot.nodes.is_empty());
    manager.shutdown().await;
}

#[tokio::test]
async fn connection_failures_are_bounded() {
    let _serial = SERIAL.lock().await;
    // Gated like the others; nothing listens on this port of the test
    // cluster's range.
    let _ = require!("CMOM_IT_MONGOS_URI");
    let mut config = config("mongodb://localhost:37039/");
    config.timeout = Duration::from_secs(1);
    let started = Instant::now();
    let result = MongoManager::connect(config).await;
    let elapsed = started.elapsed();
    match result {
        Err(MongoOpsError::Connection(message)) => {
            assert!(message.contains("localhost:37039"), "{message}")
        }
        Err(MongoOpsError::Timeout { .. }) => {}
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("connected to nothing"),
    }
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

#[tokio::test]
async fn lists_operations_through_mongos() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(config(&uri)).await;
    let load = start_load(&uri, "cmom-it-load-list").await;

    let (_, snapshot) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o) && shard_of(o) == Some("shard02")
    })
    .await;
    let mine: Vec<&Operation> = snapshot
        .operations
        .iter()
        .filter(|o| load.is_mine(o))
        .collect();
    let mut shards: Vec<&str> = mine.iter().filter_map(|o| shard_of(o)).collect();
    shards.sort_unstable();
    assert_eq!(shards, ["shard01", "shard02"], "{mine:#?}");
    for op in &mine {
        assert_eq!(op.source, OpSource::Main);
        let (shard, _) = op.opid.shard_parts().expect("shard:opid");
        assert_eq!(Some(shard), op.shard.as_deref());
        assert_eq!(op.key.0, op.opid.to_string());
        let expected_host = if shard == "shard01" {
            "localhost:37021"
        } else {
            "localhost:37024"
        };
        assert_eq!(op.host.as_deref(), Some(expected_host));
        assert_eq!(op.mongos_host.as_deref(), Some("localhost:37017"));
        assert_eq!(op.ns, "cmomit.load");
        assert_eq!(op.op, "query");
        assert!(!op.client.is_empty());
        assert!(op.current_op_time.is_some());
        assert_eq!(op.node_role, None);
    }

    // Our own $currentOp is hidden.
    assert!(
        !snapshot
            .operations
            .iter()
            .any(|o| o.app_name.as_deref() == Some(APP_NAME)),
        "our own operations are listed"
    );

    // The start time is stable across refreshes.
    let first = mine[0].clone();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let again = load_ops(&manager, &FetchQuery::default(), &load).await;
    let again = again
        .iter()
        .find(|o| o.key == first.key)
        .expect("still running");
    let start = |op: &Operation| {
        chrono::DateTime::parse_from_rfc3339(op.current_op_time.as_deref().unwrap()).unwrap()
    };
    let drift = (start(&first) - start(again)).abs();
    assert!(drift < chrono::TimeDelta::milliseconds(50), "{drift:?}");
    assert!(again.running_micros() > first.running_micros());

    // The mongos' own operation, on request.
    let (local, _) = wait_for(
        &manager,
        &with_mongos_local(),
        "the mongos operation",
        |o| load.is_mine(o) && o.source == OpSource::MongosLocal,
    )
    .await;
    assert!(matches!(local.opid, OpId::Num(_)));
    assert_eq!(local.host.as_deref(), Some("localhost:37017"));
    assert_eq!(
        local.key.0,
        format!("mongos/localhost:37017/{}", local.opid)
    );

    // Without the system filter, our own operations show up.
    let unfiltered = connect(ConnectConfig {
        hide_system_ops: false,
        ..config(&uri)
    })
    .await;
    let snapshot = unfiltered.fetch(&FetchQuery::default()).await.unwrap();
    assert!(
        snapshot
            .operations
            .iter()
            .any(|o| o.app_name.as_deref() == Some(APP_NAME)),
        "our own $currentOp is not listed without the system filter"
    );

    unfiltered.shutdown().await;
    load.stop().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn filters_apply_on_the_server() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(config(&uri)).await;
    let load = start_load(&uri, "cmom-it-load-filters").await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o) && shard_of(o) == Some("shard01") && o.secs_running >= 2
    })
    .await;
    let listed = |ops: &[Operation]| ops.iter().any(|o| o.key == op.key);

    let filters = |f: Filters| with_filters(f);
    let client = Filters {
        client: op.client.clone(),
        ..Filters::default()
    };
    assert!(listed(&load_ops(&manager, &filters(client), &load).await));
    let no_client = Filters {
        client: "zz-no-such-client".into(),
        ..Filters::default()
    };
    assert!(
        load_ops(&manager, &filters(no_client), &load)
            .await
            .is_empty()
    );

    let (_, number) = op.opid.shard_parts().unwrap();
    for opid in [number.to_string(), op.opid.to_string()] {
        let by_opid = Filters {
            opid,
            ..Filters::default()
        };
        assert!(listed(&load_ops(&manager, &filters(by_opid), &load).await));
    }
    let other_opid = Filters {
        opid: "shard09:".into(),
        ..Filters::default()
    };
    assert!(
        load_ops(&manager, &filters(other_opid), &load)
            .await
            .is_empty()
    );

    for (running_time, expected) in [("1", true), ("100000", false), ("abc", true)] {
        let running = Filters {
            running_time: running_time.into(),
            ..Filters::default()
        };
        assert_eq!(
            listed(&load_ops(&manager, &filters(running), &load).await),
            expected,
            "running_time {running_time}"
        );
    }

    let description = Filters {
        description: op.desc.to_uppercase(),
        operation: "QUERY".into(),
        ..Filters::default()
    };
    assert!(listed(
        &load_ops(&manager, &filters(description), &load).await
    ));
    let insert = Filters {
        operation: "insert".into(),
        ..Filters::default()
    };
    assert!(load_ops(&manager, &filters(insert), &load).await.is_empty());
    // No authentication: no effective users.
    let users = Filters {
        effective_users: "alice".into(),
        ..Filters::default()
    };
    assert!(load_ops(&manager, &filters(users), &load).await.is_empty());

    for (namespace, expected) in [("CMOMIT.lo", true), ("cmomit.other", false)] {
        let scoped = connect(ConnectConfig {
            namespace: namespace.into(),
            ..config(&uri)
        })
        .await;
        assert_eq!(
            listed(&load_ops(&scoped, &FetchQuery::default(), &load).await),
            expected,
            "namespace {namespace}"
        );
        scoped.shutdown().await;
    }

    load.stop().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn per_node_mode_lists_every_member() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(all_nodes(&uri)).await;
    assert!(manager.info().all_nodes);
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    // The arbiter of shard01 (localhost:37023) is not polled.
    assert_eq!(node_summary(&snapshot), sharded_cluster_nodes());

    // Operations running on secondaries.
    let secondary_uri = format!(
        "{}{}readPreference=secondary",
        uri,
        if uri.contains('?') { "&" } else { "?" }
    );
    let load = start_load(&secondary_uri, "cmom-it-load-secondary").await;
    let (op, snapshot) = wait_for(
        &manager,
        &FetchQuery::default(),
        "the secondary operation",
        |o| {
            load.is_mine(o)
                && o.source
                    == OpSource::Node {
                        address: "localhost:37022".into(),
                    }
        },
    )
    .await;
    assert_eq!(op.node_role, Some(NodeRole::Secondary));
    assert_eq!(op.shard.as_deref(), Some("shard01"));
    assert_eq!(op.host.as_deref(), Some("localhost:37022"));
    assert!(matches!(op.opid, OpId::Num(_)));
    assert_eq!(op.key.0, format!("localhost:37022/{}", op.opid));
    match node_health(&snapshot, "localhost:37022") {
        NodeHealth::Ok { operations, .. } => assert!(*operations >= 1),
        other => panic!("unexpected {other:?}"),
    }
    let shard02 = wait_for(
        &manager,
        &FetchQuery::default(),
        "the passive member operation",
        |o| {
            load.is_mine(o)
                && o.source
                    == OpSource::Node {
                        address: "localhost:37025".into(),
                    }
        },
    )
    .await
    .0;
    assert_eq!(shard02.shard.as_deref(), Some("shard02"));

    // The default mode only sees the primaries.
    let primaries = connect(config(&uri)).await;
    assert!(
        load_ops(&primaries, &FetchQuery::default(), &load)
            .await
            .is_empty()
    );
    primaries.shutdown().await;

    load.stop().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn per_node_mode_on_a_replica_set() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_RS_URI");
    let manager = connect(all_nodes(&uri)).await;
    assert!(manager.info().all_nodes);
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    assert_eq!(
        node_summary(&snapshot),
        [
            (
                "localhost:37021".to_owned(),
                "shard01".to_owned(),
                NodeRole::Primary,
                true
            ),
            (
                "localhost:37022".to_owned(),
                "shard01".to_owned(),
                NodeRole::Secondary,
                true
            ),
        ]
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_through_mongos() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(config(&uri)).await;
    let load = start_shard01_load(&uri, "cmom-it-load-kill-mongos").await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o) && shard_of(o) == Some("shard01")
    })
    .await;
    let request = KillRequest::from_operation(&op);

    // A bare number through mongos would address a mongos operation.
    let mut numeric = request.clone();
    numeric.opid = OpId::Num(op.opid.shard_parts().unwrap().1);
    assert!(matches!(
        manager.kill(&numeric).await,
        KillOutcome::Refused(_)
    ));

    assert_eq!(manager.kill(&request).await, KillOutcome::Killed);
    // Gone through mongos, and confirmed on the shard member.
    assert_eq!(manager.kill(&request).await, KillOutcome::AlreadyFinished);
    let mut missing = request.clone();
    missing.opid = OpId::Str("shard01:2000000000".into());
    assert_eq!(manager.kill(&missing).await, KillOutcome::AlreadyFinished);
    // Not listed by mongos, and its server cannot be reached: unconfirmed.
    let mut unconfirmed = missing.clone();
    unconfirmed.host = Some("localhost:37039".into());
    match manager.kill(&unconfirmed).await {
        KillOutcome::Failed(reason) => {
            assert!(reason.starts_with("could not verify"), "{reason}")
        }
        other => panic!("unexpected {other:?}"),
    }

    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_on_a_member_in_per_node_mode() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(all_nodes(&uri)).await;
    let secondary_uri = format!(
        "{}{}readPreference=secondary",
        uri,
        if uri.contains('?') { "&" } else { "?" }
    );
    let load = start_shard01_load(&secondary_uri, "cmom-it-load-kill-node").await;
    let node = OpSource::Node {
        address: "localhost:37022".into(),
    };
    let (op, _) = wait_for(
        &manager,
        &FetchQuery::default(),
        "the secondary operation",
        |o| load.is_mine(o) && o.source == node,
    )
    .await;
    let request = KillRequest::from_operation(&op);
    assert_eq!(manager.kill(&request).await, KillOutcome::Killed);

    let mut missing = request.clone();
    missing.opid = OpId::Num(2_000_000_000);
    assert_eq!(manager.kill(&missing).await, KillOutcome::AlreadyFinished);

    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_on_a_replica_set() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_RS_URI");
    let manager = connect(config(&uri)).await;
    let load = start_load(&uri, "cmom-it-load-kill-rs").await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o)
    })
    .await;
    assert_eq!(op.source, OpSource::Main);
    assert!(matches!(op.opid, OpId::Num(_)));
    assert_eq!(op.host.as_deref(), Some("localhost:37021"));
    let request = KillRequest::from_operation(&op);
    assert!(request.connection.as_deref().unwrap().starts_with("conn"));

    // The opid now names an operation started at another time, on another
    // connection.
    let mut reused = request.clone();
    reused.current_op_time = Some("2020-01-01T00:00:00.000+00:00".into());
    reused.connection = Some("conn999999".into());
    assert_eq!(
        manager.kill(&reused).await,
        KillOutcome::Refused("operation id now belongs to a different operation".into())
    );
    // Listed by another server: looked up there, where it does not run.
    let mut elsewhere = request.clone();
    elsewhere.host = Some("localhost:37022".into());
    assert_eq!(manager.kill(&elsewhere).await, KillOutcome::AlreadyFinished);
    let ops = load_ops(&manager, &FetchQuery::default(), &load).await;
    assert!(
        ops.iter().any(|o| o.key == op.key),
        "the refused kills killed it"
    );

    // A start time that moved (clock step, next statement of a
    // multi-statement write) on the same connection: the same operation.
    let mut moved = request.clone();
    moved.current_op_time = Some("2020-01-01T00:00:00.000+00:00".into());
    assert_eq!(manager.kill(&moved).await, KillOutcome::Killed);
    let mut missing = request.clone();
    missing.opid = OpId::Num(2_000_000_000);
    assert_eq!(manager.kill(&missing).await, KillOutcome::AlreadyFinished);

    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_on_a_standalone_server() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_STANDALONE_URI");
    let manager = connect(config(&uri)).await;
    let load = start_load(&uri, "cmom-it-load-kill-standalone").await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o)
    })
    .await;
    assert_eq!(op.host.as_deref(), Some("localhost:37030"));
    assert_eq!(
        manager.kill(&KillRequest::from_operation(&op)).await,
        KillOutcome::Killed
    );
    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_mongos_local_operations() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let manager = connect(config(&uri)).await;
    let load = start_load(&uri, "cmom-it-load-kill-local").await;
    let (op, _) = wait_for(
        &manager,
        &with_mongos_local(),
        "the mongos operation",
        |o| load.is_mine(o) && o.source == OpSource::MongosLocal,
    )
    .await;
    assert_eq!(
        manager.kill(&KillRequest::from_operation(&op)).await,
        KillOutcome::Killed
    );
    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_mongos_local_operations_with_several_mongos() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_MULTI_URI");
    let manager = connect(config(&uri)).await;
    assert_eq!(manager.info().target, "localhost:37017 +1");
    // The operation runs on the second mongos; the main client may talk to
    // either.
    let second = last_host_uri(&uri).await.unwrap();
    let load = start_load(&second, "cmom-it-load-kill-multi").await;
    let (op, _) = wait_for(
        &manager,
        &with_mongos_local(),
        "the mongos operation",
        |o| load.is_mine(o) && o.source == OpSource::MongosLocal,
    )
    .await;
    assert_eq!(op.host.as_deref(), Some("localhost:37018"));
    let request = KillRequest::from_operation(&op);

    // A mongos that cannot be reached directly.
    let mut unreachable = request.clone();
    unreachable.host = Some("localhost:37039".into());
    match manager.kill(&unreachable).await {
        KillOutcome::Refused(reason) => {
            assert!(reason.contains("cannot be reached directly"), "{reason}")
        }
        other => panic!("unexpected {other:?}"),
    }

    assert_eq!(manager.kill(&request).await, KillOutcome::Killed);
    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn falls_back_to_mongos_when_members_cannot_be_polled() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    // The shard members reject this user (the cluster has no such user);
    // mongos and the config servers are polled with the main credentials
    // (none needed here).
    let mut credential = Credential::default();
    credential.username = Some("cmom-it-nobody".into());
    credential.password = Some("wrong".into());
    credential.source = Some("admin".into());
    let manager = connect(ConnectConfig {
        node_credential: Some(credential),
        ..all_nodes(&uri)
    })
    .await;
    let load = start_load(&uri, "cmom-it-load-fallback").await;
    let (op, snapshot) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o) && shard_of(o) == Some("shard01")
    })
    .await;
    // Listed through mongos instead.
    assert_eq!(op.source, OpSource::Main);
    let check_nodes = |snapshot: &Snapshot| {
        for node in &snapshot.nodes {
            match (&node.health, node.shard.as_deref()) {
                (NodeHealth::Fallback { error }, Some("shard01" | "shard02")) => {
                    assert!(error.contains("Authentication failed"), "{error}");
                    assert!(error.contains("check --node-username"), "{error}");
                }
                (NodeHealth::Ok { .. }, Some("config")) => {}
                other => panic!("unexpected {other:?} for {}", node.address),
            }
        }
    };
    check_nodes(&snapshot);

    // Known to fail now: listed through mongos along with the polls,
    // without waiting for the members.
    let started = Instant::now();
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
    check_nodes(&snapshot);
    let shards: Vec<Option<&str>> = snapshot
        .operations
        .iter()
        .filter(|o| load.is_mine(o))
        .map(shard_of)
        .collect();
    assert!(shards.contains(&Some("shard01")), "{shards:?}");
    assert!(shards.contains(&Some("shard02")), "{shards:?}");
    load.stop().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_mongos_local_operations_on_the_mongos_that_listed_them() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let multi = require!("CMOM_IT_MONGOS_MULTI_URI");
    let second = last_host_uri(&multi).await.unwrap();
    // `manager` has one address, which leads to another mongos than the one
    // running the operation (as a load balancer could).
    let manager = connect(config(&uri)).await;
    let lister = connect(config(&second)).await;
    let load = start_load(&second, "cmom-it-load-kill-other-mongos").await;
    let (op, _) = wait_for(&lister, &with_mongos_local(), "the mongos operation", |o| {
        load.is_mine(o) && o.source == OpSource::MongosLocal
    })
    .await;
    assert_eq!(op.host.as_deref(), Some("localhost:37018"));
    let request = KillRequest::from_operation(&op);

    let mut unreachable = request.clone();
    unreachable.host = Some("localhost:37039".into());
    match manager.kill(&unreachable).await {
        KillOutcome::Refused(reason) => {
            assert!(reason.contains("cannot be reached directly"), "{reason}")
        }
        other => panic!("unexpected {other:?}"),
    }

    assert_eq!(manager.kill(&request).await, KillOutcome::Killed);
    load.expect_interrupted().await;
    lister.shutdown().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_on_a_server_known_by_another_name() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_PLAIN_URI");
    let data = Client::with_uri_str(&uri).await.unwrap();
    let collection = data.database("cmomit").collection::<Document>("load");
    if collection.count_documents(doc! {}).await.unwrap() < 500 {
        collection.delete_many(doc! {}).await.unwrap();
        let docs: Vec<Document> = (0..500).map(|k| doc! { "k": k }).collect();
        collection.insert_many(docs).await.unwrap();
    }
    data.shutdown().await;

    let manager = connect(config(&uri)).await;
    let load = start_load(&uri, "cmom-it-load-kill-plain").await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o)
    })
    .await;
    // The container's host name, not the address connected to.
    let host = op.host.clone().unwrap();
    assert!(!host.starts_with("localhost"), "{host}");

    let request = KillRequest::from_operation(&op);
    let mut missing = request.clone();
    missing.opid = OpId::Num(2_000_000_000);
    assert_eq!(manager.kill(&missing).await, KillOutcome::AlreadyFinished);
    assert_eq!(manager.kill(&request).await, KillOutcome::Killed);
    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn the_connection_string_app_name_does_not_hide_operations() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_STANDALONE_URI");
    let app = "cmom-it-load-appname";
    let load = start_load(&uri, app).await;
    let separator = if uri.contains('?') { "&" } else { "?" };
    let manager = connect(config(&format!("{uri}{separator}appName={app}"))).await;
    let (_, snapshot) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o)
    })
    .await;
    // Our own operations are still hidden.
    assert!(
        !snapshot
            .operations
            .iter()
            .any(|o| o.app_name.as_deref() == Some(APP_NAME)),
        "our own operations are listed"
    );
    load.stop().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn another_client_announcing_our_app_name_is_listed() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_STANDALONE_URI");
    // Any client can announce our application name: it must not hide its
    // operations.
    let load = start_load(&uri, APP_NAME).await;
    let manager = connect(config(&uri)).await;
    let (_, snapshot) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o) && o.ns == "cmomit.load"
    })
    .await;
    // Our own $currentOp is still hidden.
    assert!(
        snapshot
            .operations
            .iter()
            .filter(|o| load.is_mine(o))
            .all(|o| o.ns == "cmomit.load"),
        "our own operations are listed"
    );
    load.stop().await;
    manager.shutdown().await;
}

/// Steps down the primary at `address`.
async fn step_down(address: &str) {
    let primary = Client::with_uri_str(format!("mongodb://{address}/?directConnection=true"))
        .await
        .unwrap();
    // The step down closes the connection that sent it.
    let _ = primary
        .database("admin")
        .run_command(doc! { "replSetStepDown": 10, "secondaryCatchUpPeriodSecs": 5 })
        .await;
    primary.shutdown().await;
}

/// Fetches until `load` has no operation listed any more.
async fn wait_until_unlisted(manager: &MongoManager, load: &Load) {
    let deadline = Instant::now() + APPEAR_WITHIN;
    while !load_ops(manager, &FetchQuery::default(), load)
        .await
        .is_empty()
    {
        assert!(Instant::now() < deadline, "still listed");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tokio::test]
async fn kills_on_the_former_primary_after_a_failover() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_RS_URI");
    let container = require!("CMOM_IT_CONTAINER");
    let manager = connect(config(&uri)).await;
    // Reads allowed on secondaries keep running when their primary steps
    // down.
    let load = start_load(
        &format!("{uri}&readPreference=primaryPreferred"),
        "cmom-it-load-kill-stepdown",
    )
    .await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o)
    })
    .await;
    assert_eq!(op.host.as_deref(), Some("localhost:37021"));

    let _back = PrimaryBack {
        container: &container,
    };
    step_down("localhost:37021").await;
    // The main connection follows the new primary, which does not run it.
    wait_until_unlisted(&manager, &load).await;

    assert_eq!(
        manager.kill(&KillRequest::from_operation(&op)).await,
        KillOutcome::Killed
    );
    load.expect_interrupted().await;
    manager.shutdown().await;
}

#[tokio::test]
async fn kills_through_mongos_on_the_former_primary_after_a_failover() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let container = require!("CMOM_IT_CONTAINER");
    let manager = connect(config(&uri)).await;
    let separator = if uri.contains('?') { "&" } else { "?" };
    let load = start_shard01_load(
        &format!("{uri}{separator}readPreference=primaryPreferred"),
        "cmom-it-load-kill-mongos-stepdown",
    )
    .await;
    let (op, _) = wait_for(&manager, &FetchQuery::default(), "the load", |o| {
        load.is_mine(o) && shard_of(o) == Some("shard01")
    })
    .await;
    assert_eq!(op.host.as_deref(), Some("localhost:37021"));

    let _back = PrimaryBack {
        container: &container,
    };
    step_down("localhost:37021").await;
    // mongos lists the new primary's operations only.
    wait_until_unlisted(&manager, &load).await;

    assert_eq!(
        manager.kill(&KillRequest::from_operation(&op)).await,
        KillOutcome::Killed
    );
    load.expect_interrupted().await;
    manager.shutdown().await;
}

/// Waits for `localhost:37021` to be the primary of shard01 again (it has the
/// highest priority), when dropped.
struct PrimaryBack<'a> {
    container: &'a str,
}

impl Drop for PrimaryBack<'_> {
    fn drop(&mut self) {
        cluster_node(self.container, &["wait-primary", "37021"]);
        cluster_node(self.container, &["wait-secondary", "37022"]);
    }
}

#[tokio::test]
async fn follows_a_failover_in_per_node_mode() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let container = require!("CMOM_IT_CONTAINER");
    let manager = connect(all_nodes(&uri)).await;
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    assert_eq!(node_summary(&snapshot), sharded_cluster_nodes());

    let primary = Client::with_uri_str("mongodb://localhost:37021/?directConnection=true")
        .await
        .unwrap();
    let _back = PrimaryBack {
        container: &container,
    };
    // The step down closes the connection that sent it.
    let _ = primary
        .database("admin")
        .run_command(doc! { "replSetStepDown": 15, "secondaryCatchUpPeriodSecs": 5 })
        .await;

    // Roles are refreshed on every poll: no fallback, no warning.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
        let roles: Vec<(String, NodeRole)> = snapshot
            .nodes
            .iter()
            .filter(|n| n.shard.as_deref() == Some("shard01"))
            .map(|n| (n.address.clone(), n.role))
            .collect();
        if roles
            == [
                ("localhost:37021".to_owned(), NodeRole::Secondary),
                ("localhost:37022".to_owned(), NodeRole::Primary),
            ]
        {
            assert!(
                snapshot.nodes.iter().all(|n| n.health.is_ok()),
                "{:?}",
                snapshot.nodes
            );
            assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
            break;
        }
        assert!(Instant::now() < deadline, "no failover seen: {roles:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    primary.shutdown().await;
    manager.shutdown().await;
}

/// Runs `node.sh` in the test container.
fn cluster_node(container: &str, args: &[&str]) {
    let status = Command::new("docker")
        .args(["exec", container, "bash", "/cmom/node.sh"])
        .args(args)
        .status()
        .expect("docker exec");
    assert!(status.success(), "node.sh {args:?} failed");
}

/// Restarts a stopped member when dropped, even if the test fails.
struct Restart<'a> {
    container: &'a str,
    port: &'a str,
}

impl Drop for Restart<'_> {
    fn drop(&mut self) {
        cluster_node(self.container, &["start", self.port]);
        cluster_node(self.container, &["wait-secondary", self.port]);
    }
}

#[tokio::test]
async fn reports_a_member_that_is_down() {
    let _serial = SERIAL.lock().await;
    let uri = require!("CMOM_IT_MONGOS_URI");
    let container = require!("CMOM_IT_CONTAINER");
    let manager = connect(all_nodes(&uri)).await;
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    assert!(
        snapshot.nodes.iter().all(|n| n.health.is_ok()),
        "{:?}",
        snapshot.nodes
    );

    cluster_node(&container, &["stop", "37022"]);
    let restart = Restart {
        container: &container,
        port: "37022",
    };

    let started = Instant::now();
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < TIMEOUT + Duration::from_secs(2),
        "took {elapsed:?}"
    );
    match node_health(&snapshot, "localhost:37022") {
        NodeHealth::Failed { error } => assert!(!error.is_empty()),
        other => panic!("unexpected {other:?}"),
    }
    for node in snapshot
        .nodes
        .iter()
        .filter(|n| n.address != "localhost:37022")
    {
        assert!(node.health.is_ok(), "{node:?}");
    }

    // Known to be down: the next refreshes do not wait for it.
    let started = Instant::now();
    let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
    assert!(matches!(
        node_health(&snapshot, "localhost:37022"),
        NodeHealth::Failed { .. }
    ));

    // Connecting while it is down: still reported, role unknown.
    let fresh = connect(all_nodes(&uri)).await;
    let snapshot = fresh.fetch(&FetchQuery::default()).await.unwrap();
    let down = snapshot
        .nodes
        .iter()
        .find(|n| n.address == "localhost:37022")
        .expect("the stopped member is reported");
    assert!(matches!(down.health, NodeHealth::Failed { .. }), "{down:?}");
    assert_eq!(down.role, NodeRole::Unknown);
    fresh.shutdown().await;

    drop(restart);

    // Back to normal.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let snapshot = manager.fetch(&FetchQuery::default()).await.unwrap();
        if node_health(&snapshot, "localhost:37022").is_ok() {
            assert_eq!(node_summary(&snapshot), sharded_cluster_nodes());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the member did not come back: {:?}",
            snapshot.nodes
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    manager.shutdown().await;
}
