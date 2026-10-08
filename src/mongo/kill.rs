//! Killing operations safely.
//!
//! Opids are per-server counters and get reused (after a restart, or on
//! another shard primary after a failover). An operation is identified by its
//! opid, the server running it, and its start time or connection. The kill is
//! sent to the server that listed the operation, only after checking that the
//! opid still belongs to the same operation, and verified.

use std::future::Future;
use std::time::Duration;

use mongodb::Client;
use mongodb::bson::{Bson, Document, doc};

use super::call::{admin_aggregate, admin_command};
use super::parse::{as_i64, same_start_time, start_time};
use super::pipeline::lookup_pipeline;
use crate::model::{KillOutcome, KillRequest, OpId, OpSource};

/// Why a kill was refused after a lookup.
const DIFFERENT_OPERATION: &str = "operation id now belongs to a different operation";

/// Where and how to kill one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KillPlan {
    /// Through mongos, which routes `"shard:opid"` to the shard primary.
    /// `number` is the opid on that shard.
    Mongos { shard: String, number: i64 },
    /// On the mongod that listed it (standalone or replica set member),
    /// whose own name for itself is `host`.
    Mongod { host: String },
    /// On the mongos that listed it: one of its own operations.
    MongosLocal { host: String },
    /// On the cluster member that listed it (`--all-nodes`).
    Member { address: String },
}

/// Chooses where to send a kill, or explains why it must not be sent.
///
/// A kill always goes to the server that listed the operation: a replica set
/// connection follows the current primary, and one address can front
/// several mongos (a load balancer, a Kubernetes service, round-robin DNS),
/// so the lookup, the kill and the verification could reach different
/// servers otherwise.
pub(crate) fn plan_kill(request: &KillRequest, sharded: bool) -> Result<KillPlan, String> {
    let opid = &request.opid;
    let host = request.host.as_deref().filter(|h| !h.is_empty());
    match &request.source {
        OpSource::Main if sharded => {
            // Never strip the shard prefix: on mongos a bare number addresses
            // the mongos' own operations, i.e. a different operation.
            let (shard, number) = opid.shard_parts().ok_or_else(|| {
                format!(
                    "operation id {opid} has no shard prefix; through mongos it would \
                     address one of the mongos' own operations"
                )
            })?;
            Ok(KillPlan::Mongos {
                shard: shard.to_owned(),
                number,
            })
        }
        OpSource::Main => {
            require_numeric(opid)?;
            let host = host.ok_or("the server running the operation is unknown")?;
            Ok(KillPlan::Mongod {
                host: host.to_owned(),
            })
        }
        OpSource::MongosLocal => {
            if !sharded {
                return Err("not connected to mongos".into());
            }
            require_numeric(opid)?;
            let host = host.ok_or("the mongos running the operation is unknown")?;
            Ok(KillPlan::MongosLocal {
                host: host.to_owned(),
            })
        }
        OpSource::Node { address } => {
            require_numeric(opid)?;
            Ok(KillPlan::Member {
                address: address.clone(),
            })
        }
    }
}

fn require_numeric(opid: &OpId) -> Result<(), String> {
    match opid {
        OpId::Num(_) => Ok(()),
        OpId::Str(_) => Err(format!("unexpected operation id {opid} for this server")),
    }
}

/// An operation found by the lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FoundOp {
    pub host: Option<String>,
    /// Start time, as computed by [`start_time`].
    pub start: Option<String>,
    /// `desc`, e.g. `"conn123"`.
    pub connection: Option<String>,
    pub connection_id: Option<i64>,
    pub kill_pending: bool,
}

impl FoundOp {
    pub fn from_doc(doc: &Document) -> Self {
        let string = |key: &str| {
            doc.get_str(key)
                .ok()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            host: string("host"),
            start: start_time(doc),
            connection: string("desc"),
            connection_id: doc.get("connectionId").and_then(as_i64),
            kill_pending: matches!(doc.get("killPending"), Some(Bson::Boolean(true))),
        }
    }
}

/// Whether `found`, which has the opid of `request`, is the same operation:
/// on the same host (when both are known), and with the same start time
/// (within a second) or on the same connection.
///
/// The start time alone is not enough: it moves when the wall clock is
/// stepped, and a multi-statement update or delete restarts
/// `microsecs_running` with every statement.
pub(crate) fn is_same_operation(request: &KillRequest, found: &FoundOp) -> bool {
    if let (Some(listed), Some(now)) = (&request.host, &found.host)
        && !listed.eq_ignore_ascii_case(now)
    {
        return false;
    }
    let start = match (&request.current_op_time, &found.start) {
        (Some(listed), Some(now)) => Some(same_start_time(listed, now)),
        _ => None,
    };
    let connection = same_connection(request.connection.as_deref(), found);
    match (start, connection) {
        // Nothing to tell them apart.
        (None, None) => true,
        _ => start == Some(true) || connection == Some(true),
    }
}

/// Whether `found` runs on the connection `listed` (`desc`), comparing its
/// `desc` and `connectionId` when present. `None` when it cannot be told.
fn same_connection(listed: Option<&str>, found: &FoundOp) -> Option<bool> {
    let listed = listed?;
    let by_desc = found.connection.as_deref().map(|now| now == listed);
    let by_id = found
        .connection_id
        .zip(connection_number(listed))
        .map(|(now, listed)| now == listed);
    match (by_desc, by_id) {
        (None, None) => None,
        (by_desc, by_id) => Some(by_desc != Some(false) && by_id != Some(false)),
    }
}

/// `123` for `"conn123"`.
fn connection_number(desc: &str) -> Option<i64> {
    desc.strip_prefix("conn")?.parse().ok()
}

/// Polling of the operation after the kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KillTiming {
    pub poll_interval: Duration,
    /// How long to wait for the operation to go away after each killOp.
    pub verify_window: Duration,
}

impl Default for KillTiming {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            verify_window: Duration::from_secs(5),
        }
    }
}

/// The server side of a kill.
pub(crate) trait KillBackend {
    /// Looks the operation up by opid.
    fn lookup(&self) -> impl Future<Output = Result<Option<FoundOp>, String>> + Send;
    /// Sends killOp.
    fn kill_op(&self) -> impl Future<Output = Result<(), String>> + Send;
}

/// A real server, through `client`.
pub(crate) struct ServerBackend {
    pub client: Client,
    /// The opid as this server knows it.
    pub opid: OpId,
    pub local_ops: bool,
    pub timeout: Duration,
}

impl KillBackend for ServerBackend {
    async fn lookup(&self) -> Result<Option<FoundOp>, String> {
        let docs = admin_aggregate(
            &self.client,
            "looking up the operation",
            lookup_pipeline(&self.opid, self.local_ops),
            1,
            self.timeout,
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(docs.first().map(FoundOp::from_doc))
    }

    async fn kill_op(&self) -> Result<(), String> {
        admin_command(
            &self.client,
            "killOp",
            doc! { "killOp": 1, "op": self.opid.to_bson() },
            self.timeout,
        )
        .await
        .map(drop)
        .map_err(|e| e.to_string())
    }
}

/// What a failed first lookup means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unreachable<'a> {
    /// The kill failed.
    Fail,
    /// The kill failed, with this explanation.
    FailWith(&'a str),
    /// The kill must not be sent another way: refused, with this
    /// explanation.
    Refuse(&'a str),
}

impl Unreachable<'_> {
    fn outcome(self, error: String) -> KillOutcome {
        match self {
            Self::Fail => KillOutcome::Failed(error),
            Self::FailWith(explanation) => KillOutcome::Failed(format!("{explanation}: {error}")),
            Self::Refuse(explanation) => KillOutcome::Refused(format!("{explanation}: {error}")),
        }
    }
}

/// Result of looking the operation up.
enum Lookup {
    Running,
    NotFound,
    /// Another operation has the opid now.
    Different,
    Error(String),
}

async fn look_up<B: KillBackend>(backend: &B, request: &KillRequest) -> Lookup {
    match backend.lookup().await {
        Err(error) => Lookup::Error(error),
        Ok(None) => Lookup::NotFound,
        Ok(Some(found)) if is_same_operation(request, &found) => Lookup::Running,
        Ok(Some(_)) => Lookup::Different,
    }
}

/// Kills the operation of `request` on the server behind `backend`.
pub(crate) async fn execute<B: KillBackend>(
    backend: &B,
    request: &KillRequest,
    timing: KillTiming,
    unreachable: Unreachable<'_>,
) -> KillOutcome {
    match look_up(backend, request).await {
        Lookup::Error(error) => unreachable.outcome(error),
        Lookup::NotFound => KillOutcome::AlreadyFinished,
        Lookup::Different => KillOutcome::Refused(DIFFERENT_OPERATION.into()),
        Lookup::Running => kill_and_verify(backend, request, timing).await,
    }
}

/// Explains a [`KillPlan::Mongos`] kill that could not be confirmed.
pub(crate) const UNVERIFIED: &str =
    "could not verify that the operation is gone (mongos does not list it any more)";

/// Kills a shard operation through mongos.
///
/// When mongos does not list it any more, the operation may still run on
/// the server that listed it, e.g. a former primary after a failover (mongos
/// only looks at the current one): `member`, a connection to that server,
/// confirms it is gone, or kills it there.
pub(crate) async fn execute_through_mongos<M, D>(
    mongos: &M,
    member: impl Future<Output = Result<D, String>>,
    request: &KillRequest,
    timing: KillTiming,
) -> KillOutcome
where
    M: KillBackend,
    D: KillBackend,
{
    match look_up(mongos, request).await {
        Lookup::Error(error) => KillOutcome::Failed(error),
        Lookup::Different => KillOutcome::Refused(DIFFERENT_OPERATION.into()),
        Lookup::Running => kill_and_verify(mongos, request, timing).await,
        Lookup::NotFound => match member.await {
            Ok(member) => {
                execute(&member, request, timing, Unreachable::FailWith(UNVERIFIED)).await
            }
            Err(error) => KillOutcome::Failed(format!("{UNVERIFIED}: {error}")),
        },
    }
}

/// Result of waiting for a killed operation to go away.
enum Verification {
    Gone,
    StillRunning {
        /// The operation is marked for killing already.
        kill_pending: bool,
    },
    /// The lookup failed every time.
    Unknown(String),
}

async fn kill_and_verify<B: KillBackend>(
    backend: &B,
    request: &KillRequest,
    timing: KillTiming,
) -> KillOutcome {
    let mut unknown = None;
    for attempt in 1..=2 {
        if let Err(error) = backend.kill_op().await {
            return KillOutcome::Failed(error);
        }
        match verify(backend, request, timing).await {
            Verification::Gone => return KillOutcome::Killed,
            Verification::StillRunning { kill_pending } => {
                log::warn!(
                    "Operation {} still running after kill attempt {attempt}{}",
                    request.opid,
                    if kill_pending { " (kill pending)" } else { "" }
                );
                unknown = None;
                // Marked for killing: it ends at its next interrupt point,
                // another killOp would not hasten it.
                if kill_pending {
                    break;
                }
            }
            Verification::Unknown(error) => unknown = Some(error),
        }
    }
    match unknown {
        Some(error) => KillOutcome::Failed(format!(
            "killOp was sent but its effect could not be verified: {error}"
        )),
        None => KillOutcome::StillRunning,
    }
}

async fn verify<B: KillBackend>(
    backend: &B,
    request: &KillRequest,
    timing: KillTiming,
) -> Verification {
    let deadline = tokio::time::Instant::now() + timing.verify_window;
    let mut seen = None;
    let mut last_error = None;
    loop {
        match backend.lookup().await {
            Ok(None) => return Verification::Gone,
            Ok(Some(found)) if is_same_operation(request, &found) => {
                seen = Some(found.kill_pending);
            }
            // Another operation got the opid: ours is gone.
            Ok(Some(_)) => return Verification::Gone,
            Err(error) => last_error = Some(error),
        }
        if tokio::time::Instant::now() + timing.poll_interval > deadline {
            break;
        }
        tokio::time::sleep(timing.poll_interval).await;
    }
    match (seen, last_error) {
        (Some(kill_pending), _) => Verification::StillRunning { kill_pending },
        (None, Some(error)) => Verification::Unknown(error),
        (None, None) => Verification::StillRunning {
            kill_pending: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::model::OpKey;

    const START: &str = "2026-10-08T16:29:11.101+00:00";

    fn request(source: OpSource, opid: OpId) -> KillRequest {
        KillRequest {
            key: OpKey::new(&source, &opid, Some("db1:27017")),
            opid,
            source,
            host: Some("db1:27017".into()),
            current_op_time: Some(START.into()),
            connection: Some("conn7".into()),
            description: "ns=app.users op=query".into(),
        }
    }

    fn node(address: &str) -> OpSource {
        OpSource::Node {
            address: address.into(),
        }
    }

    #[test]
    fn sharded_main_operations_go_through_mongos_with_the_shard_prefix() {
        let plan = plan_kill(
            &request(OpSource::Main, OpId::Str("shard01:42".into())),
            true,
        );
        assert_eq!(
            plan,
            Ok(KillPlan::Mongos {
                shard: "shard01".into(),
                number: 42
            })
        );
        // A bare number through mongos would be a mongos operation.
        let refused = plan_kill(&request(OpSource::Main, OpId::Num(42)), true).unwrap_err();
        assert!(refused.contains("no shard prefix"), "{refused}");
        assert!(plan_kill(&request(OpSource::Main, OpId::Str("weird".into())), true).is_err());
    }

    #[test]
    fn replica_set_main_operations_go_to_the_server_that_listed_them() {
        let plan = plan_kill(&request(OpSource::Main, OpId::Num(42)), false);
        assert_eq!(
            plan,
            Ok(KillPlan::Mongod {
                host: "db1:27017".into()
            })
        );
        let mut unknown_host = request(OpSource::Main, OpId::Num(42));
        unknown_host.host = Some(String::new());
        assert!(plan_kill(&unknown_host, false).is_err());
        assert!(plan_kill(&request(OpSource::Main, OpId::Str("s:1".into())), false).is_err());
    }

    #[test]
    fn mongos_local_operations_go_to_the_mongos_that_listed_them() {
        let req = request(OpSource::MongosLocal, OpId::Num(7));
        // Even with a single address for the main connection: it may front
        // several mongos.
        assert_eq!(
            plan_kill(&req, true),
            Ok(KillPlan::MongosLocal {
                host: "db1:27017".into()
            })
        );
        let mut unknown_host = req.clone();
        unknown_host.host = None;
        assert!(plan_kill(&unknown_host, true).is_err());
        assert!(plan_kill(&req, false).is_err());
        let string_opid = request(OpSource::MongosLocal, OpId::Str("s:1".into()));
        assert!(plan_kill(&string_opid, true).is_err());
    }

    #[test]
    fn node_operations_go_to_the_member() {
        let plan = plan_kill(&request(node("db2:27018"), OpId::Num(9)), true);
        assert_eq!(
            plan,
            Ok(KillPlan::Member {
                address: "db2:27018".into()
            })
        );
        assert!(plan_kill(&request(node("db2:27018"), OpId::Str("s:9".into())), true).is_err());
    }

    fn found(host: &str, start: &str, connection: &str) -> FoundOp {
        FoundOp {
            host: Some(host.into()),
            start: Some(start.into()),
            connection: Some(connection.into()),
            connection_id: connection_number(connection),
            kill_pending: false,
        }
    }

    const LATER: &str = "2026-10-08T16:35:00.000+00:00";

    #[test]
    fn identity_needs_the_host_and_the_start_time_or_the_connection() {
        let req = request(OpSource::Main, OpId::Num(1));
        let same = found("DB1:27017", "2026-10-08T16:29:11.105+00:00", "conn7");
        assert!(is_same_operation(&req, &same));
        assert!(is_same_operation(&req, &FoundOp::default()));
        // The start time moved (clock step, next statement): same connection.
        assert!(is_same_operation(&req, &found("db1:27017", LATER, "conn7")));
        // Same start time, other connection: e.g. desc not comparable.
        assert!(is_same_operation(&req, &found("db1:27017", START, "conn8")));
        // Neither.
        assert!(!is_same_operation(
            &req,
            &found("db1:27017", LATER, "conn8")
        ));
        // Another host is always another operation.
        assert!(!is_same_operation(
            &req,
            &found("db9:27017", START, "conn7")
        ));
        let mut unlisted = req.clone();
        unlisted.current_op_time = None;
        unlisted.connection = None;
        unlisted.host = None;
        assert!(is_same_operation(
            &unlisted,
            &found("db9:27017", LATER, "conn8")
        ));
    }

    #[test]
    fn identity_with_partial_information() {
        let req = request(OpSource::Main, OpId::Num(1));
        // Only the start time is known.
        let no_connection = FoundOp {
            connection: None,
            connection_id: None,
            ..found("db1:27017", LATER, "conn7")
        };
        assert!(!is_same_operation(&req, &no_connection));
        // Only the connection is known.
        let no_start = FoundOp {
            start: None,
            ..found("db1:27017", LATER, "conn7")
        };
        assert!(is_same_operation(&req, &no_start));
        let other_connection = FoundOp {
            start: None,
            ..found("db1:27017", LATER, "conn8")
        };
        assert!(!is_same_operation(&req, &other_connection));
        // connectionId alone, and contradicting the desc.
        let by_id = FoundOp {
            start: None,
            connection: None,
            connection_id: Some(7),
            ..FoundOp::default()
        };
        assert!(is_same_operation(&req, &by_id));
        let contradicting = FoundOp {
            connection_id: Some(9),
            ..no_start
        };
        assert!(!is_same_operation(&req, &contradicting));
    }

    #[test]
    fn found_operations_from_documents() {
        let doc = doc! {
            "opid": 7,
            "host": "db1:27017",
            "desc": "conn7",
            "connectionId": 7_i64,
            "currentOpTime": "2026-10-08T16:29:13.122+00:00",
            "microsecs_running": 2_021_000_i64,
            "killPending": true,
        };
        assert_eq!(
            FoundOp::from_doc(&doc),
            FoundOp {
                host: Some("db1:27017".into()),
                start: Some(START.into()),
                connection: Some("conn7".into()),
                connection_id: Some(7),
                kill_pending: true,
            }
        );
        assert_eq!(FoundOp::from_doc(&doc! { "opid": 7 }), FoundOp::default());
        assert_eq!(connection_number("conn123"), Some(123));
        assert_eq!(connection_number("thread1"), None);
    }

    type LookupResult = Result<Option<FoundOp>, String>;

    /// Scripted server. Lookups return the queued results (the last one
    /// repeats) until `gone_after_kills` killOps were sent, then nothing.
    struct FakeServer {
        lookups: Mutex<VecDeque<LookupResult>>,
        kill_result: Result<(), String>,
        gone_after_kills: Option<usize>,
        lookup_calls: Mutex<usize>,
        kill_calls: Mutex<usize>,
    }

    impl FakeServer {
        fn new(lookups: Vec<LookupResult>, kill_result: Result<(), String>) -> Self {
            Self {
                lookups: Mutex::new(lookups.into()),
                kill_result,
                gone_after_kills: None,
                lookup_calls: Mutex::new(0),
                kill_calls: Mutex::new(0),
            }
        }

        fn calls(&self) -> (usize, usize) {
            (
                *self.lookup_calls.lock().unwrap(),
                *self.kill_calls.lock().unwrap(),
            )
        }
    }

    impl KillBackend for FakeServer {
        async fn lookup(&self) -> LookupResult {
            *self.lookup_calls.lock().unwrap() += 1;
            if self
                .gone_after_kills
                .is_some_and(|n| *self.kill_calls.lock().unwrap() >= n)
            {
                return Ok(None);
            }
            let mut queue = self.lookups.lock().unwrap();
            if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap()
            }
        }

        async fn kill_op(&self) -> Result<(), String> {
            *self.kill_calls.lock().unwrap() += 1;
            self.kill_result.clone()
        }
    }

    const FAST: KillTiming = KillTiming {
        poll_interval: Duration::from_millis(1),
        verify_window: Duration::from_millis(20),
    };

    fn running() -> LookupResult {
        Ok(Some(found("db1:27017", START, "conn7")))
    }

    fn pending() -> LookupResult {
        Ok(Some(FoundOp {
            kill_pending: true,
            ..found("db1:27017", START, "conn7")
        }))
    }

    fn other() -> LookupResult {
        Ok(Some(found("db1:27017", LATER, "conn8")))
    }

    fn req() -> KillRequest {
        request(OpSource::Main, OpId::Num(42))
    }

    #[tokio::test]
    async fn kills_and_verifies() {
        let server = FakeServer::new(vec![running(), running(), Ok(None)], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Killed
        );
        assert_eq!(server.calls(), (3, 1));
    }

    #[tokio::test]
    async fn finished_operations_are_not_killed() {
        let server = FakeServer::new(vec![Ok(None)], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::AlreadyFinished
        );
        assert_eq!(server.calls(), (1, 0));
    }

    #[tokio::test]
    async fn reused_opids_are_refused() {
        let server = FakeServer::new(vec![other()], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Refused(DIFFERENT_OPERATION.into())
        );
        assert_eq!(server.calls(), (1, 0));
    }

    #[tokio::test]
    async fn a_moved_start_time_on_the_same_connection_is_killed() {
        let moved = Ok(Some(found("db1:27017", LATER, "conn7")));
        let server = FakeServer::new(vec![moved.clone(), moved, Ok(None)], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Killed
        );
        assert_eq!(server.calls().1, 1);
    }

    #[tokio::test]
    async fn a_different_operation_after_the_kill_means_killed() {
        let server = FakeServer::new(vec![running(), other()], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Killed
        );
    }

    #[tokio::test]
    async fn a_moved_start_time_after_the_kill_is_not_gone() {
        // Same connection: still the operation, still running.
        let moved = Ok(Some(found("db1:27017", LATER, "conn7")));
        let server = FakeServer::new(vec![running(), moved], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::StillRunning
        );
    }

    #[tokio::test]
    async fn retries_once_then_reports_still_running() {
        let server = FakeServer::new(vec![running()], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::StillRunning
        );
        let (lookups, kills) = server.calls();
        assert_eq!(kills, 2);
        assert!(lookups >= 3, "{lookups}");
    }

    #[tokio::test]
    async fn kill_pending_operations_are_not_killed_again() {
        let server = FakeServer::new(vec![running(), pending()], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::StillRunning
        );
        assert_eq!(server.calls().1, 1);
    }

    #[tokio::test]
    async fn second_attempt_can_succeed() {
        let mut server = FakeServer::new(vec![running()], Ok(()));
        server.gone_after_kills = Some(2);
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Killed
        );
        assert_eq!(server.calls().1, 2);
    }

    #[tokio::test]
    async fn kill_errors_fail() {
        let server = FakeServer::new(vec![running()], Err("killOp: Unauthorized".into()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Failed("killOp: Unauthorized".into())
        );
    }

    #[tokio::test]
    async fn lookup_errors_fail_or_refuse() {
        let server = FakeServer::new(vec![Err("timed out".into())], Ok(()));
        assert_eq!(
            execute(&server, &req(), FAST, Unreachable::Fail).await,
            KillOutcome::Failed("timed out".into())
        );
        assert_eq!(
            execute(
                &server,
                &req(),
                FAST,
                Unreachable::FailWith("cannot reach db1")
            )
            .await,
            KillOutcome::Failed("cannot reach db1: timed out".into())
        );
        assert_eq!(
            execute(
                &server,
                &req(),
                FAST,
                Unreachable::Refuse("mongos not reachable")
            )
            .await,
            KillOutcome::Refused("mongos not reachable: timed out".into())
        );
        assert_eq!(server.calls().1, 0);
    }

    #[tokio::test]
    async fn unverifiable_kills_fail() {
        let server = FakeServer::new(vec![running(), Err("network".into())], Ok(()));
        match execute(&server, &req(), FAST, Unreachable::Fail).await {
            KillOutcome::Failed(message) => assert!(message.contains("network"), "{message}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn shard_req() -> KillRequest {
        request(OpSource::Main, OpId::Str("shard01:42".into()))
    }

    async fn no_member() -> Result<FakeServer, String> {
        panic!("the member must not be needed")
    }

    #[tokio::test]
    async fn through_mongos_when_mongos_lists_it() {
        let mongos = FakeServer::new(vec![running(), Ok(None)], Ok(()));
        assert_eq!(
            execute_through_mongos(&mongos, no_member(), &shard_req(), FAST).await,
            KillOutcome::Killed
        );
        assert_eq!(mongos.calls().1, 1);
        let mongos = FakeServer::new(vec![other()], Ok(()));
        assert_eq!(
            execute_through_mongos(&mongos, no_member(), &shard_req(), FAST).await,
            KillOutcome::Refused(DIFFERENT_OPERATION.into())
        );
        let mongos = FakeServer::new(vec![Err("mongos down".into())], Ok(()));
        assert_eq!(
            execute_through_mongos(&mongos, no_member(), &shard_req(), FAST).await,
            KillOutcome::Failed("mongos down".into())
        );
    }

    #[tokio::test]
    async fn through_mongos_absence_is_confirmed_on_the_member() {
        let mongos = FakeServer::new(vec![Ok(None)], Ok(()));
        let member = async { Ok(FakeServer::new(vec![Ok(None)], Ok(()))) };
        assert_eq!(
            execute_through_mongos(&mongos, member, &shard_req(), FAST).await,
            KillOutcome::AlreadyFinished
        );
        assert_eq!(mongos.calls(), (1, 0));
    }

    #[tokio::test]
    async fn through_mongos_a_former_primary_operation_is_killed_on_the_member() {
        let mongos = FakeServer::new(vec![Ok(None)], Ok(()));
        let member = FakeServer::new(vec![running(), Ok(None)], Ok(()));
        let outcome =
            execute_through_mongos(&mongos, async { Ok(&member) }, &shard_req(), FAST).await;
        assert_eq!(outcome, KillOutcome::Killed);
        assert_eq!(mongos.calls().1, 0);
        assert_eq!(member.calls().1, 1);
    }

    #[tokio::test]
    async fn through_mongos_unconfirmed_absence_fails() {
        let mongos = FakeServer::new(vec![Ok(None)], Ok(()));
        let member = async { Ok(FakeServer::new(vec![Err("refused".into())], Ok(()))) };
        assert_eq!(
            execute_through_mongos(&mongos, member, &shard_req(), FAST).await,
            KillOutcome::Failed(format!("{UNVERIFIED}: refused"))
        );
        let no_client = async { Err::<FakeServer, _>("invalid address".to_owned()) };
        assert_eq!(
            execute_through_mongos(&mongos, no_client, &shard_req(), FAST).await,
            KillOutcome::Failed(format!("{UNVERIFIED}: invalid address"))
        );
    }

    impl<B: KillBackend + Sync> KillBackend for &B {
        async fn lookup(&self) -> LookupResult {
            (**self).lookup().await
        }

        async fn kill_op(&self) -> Result<(), String> {
            (**self).kill_op().await
        }
    }

    #[tokio::test]
    async fn default_timing_polls_every_half_second_for_five_seconds() {
        let timing = KillTiming::default();
        assert_eq!(timing.poll_interval, Duration::from_millis(500));
        assert_eq!(timing.verify_window, Duration::from_secs(5));
    }
}
