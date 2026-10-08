//! Domain types shared by the MongoDB layer, the application state and the UI.

use std::fmt;
use std::time::Duration;

use mongodb::bson::{Bson, Document};

/// Maximum number of operations kept per refresh. The slowest operations are
/// kept when more match.
pub const MAX_OPERATIONS: usize = 1000;

/// Operation id as reported by `$currentOp`.
///
/// Opids are per-server counters: numeric on a mongod (and for a mongos' own
/// operations), and `"<shard>:<opid>"` strings when listed through mongos.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OpId {
    Num(i64),
    Str(String),
}

impl OpId {
    /// Parses the `opid` field of a `$currentOp` document.
    pub fn from_bson(value: &Bson) -> Option<Self> {
        match value {
            Bson::Int32(v) => Some(Self::Num(i64::from(*v))),
            Bson::Int64(v) => Some(Self::Num(*v)),
            Bson::Double(v) if v.is_finite() && v.fract() == 0.0 => Some(Self::Num(*v as i64)),
            Bson::String(s) => {
                let s = s.trim();
                (!s.is_empty()).then(|| Self::Str(s.to_owned()))
            }
            _ => None,
        }
    }

    /// The value to pass as killOp's `op` argument and to match `$currentOp`
    /// output with.
    pub fn to_bson(&self) -> Bson {
        match self {
            Self::Num(n) => i32::try_from(*n).map_or(Bson::Int64(*n), Bson::Int32),
            Self::Str(s) => Bson::String(s.clone()),
        }
    }

    /// For `"<shard>:<opid>"` ids, the shard name and the shard-local opid.
    pub fn shard_parts(&self) -> Option<(&str, i64)> {
        match self {
            Self::Num(_) => None,
            Self::Str(s) => {
                let (shard, num) = s.rsplit_once(':')?;
                let num = num.parse().ok()?;
                (!shard.is_empty()).then_some((shard, num))
            }
        }
    }
}

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Num(n) => write!(f, "{n}"),
            Self::Str(s) => f.write_str(s),
        }
    }
}

/// Unique key of an operation row. Stable across refreshes for as long as the
/// operation runs; used to preserve selection and cursor position.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpKey(pub String);

impl OpKey {
    /// Builds the key of an operation listed from `source`.
    ///
    /// * `Main`: the opid itself (`"123"`, or `"shard01:123"` through mongos).
    /// * `MongosLocal`: `"mongos/<host>/<opid>"`.
    /// * `Node`: `"<address>/<opid>"`.
    pub fn new(source: &OpSource, opid: &OpId, host: Option<&str>) -> Self {
        match source {
            OpSource::Main => Self(opid.to_string()),
            OpSource::MongosLocal => Self(format!("mongos/{}/{opid}", host.unwrap_or("?"))),
            OpSource::Node { address } => Self(format!("{address}/{opid}")),
        }
    }
}

impl fmt::Display for OpKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where an operation was listed, which is also where it must be killed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OpSource {
    /// Listed through the main connection: the standalone server, the replica
    /// set primary, or every shard primary when connected to mongos.
    Main,
    /// A mongos' own operation (`$currentOp` with `localOps: true`), listed
    /// through the main connection.
    MongosLocal,
    /// Listed by polling one cluster member directly (`--all-nodes`).
    Node { address: String },
}

/// Role of a cluster member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeRole {
    Primary,
    Secondary,
    Arbiter,
    Mongos,
    Standalone,
    /// Any other replica set state (startup, recovering, rollback, ...).
    Other,
    /// Not known yet (e.g. the node has never answered).
    Unknown,
}

impl NodeRole {
    /// Short label for table cells.
    pub fn short(self) -> &'static str {
        match self {
            Self::Primary => "P",
            Self::Secondary => "S",
            Self::Arbiter => "A",
            Self::Mongos => "mongos",
            Self::Standalone => "standalone",
            Self::Other => "other",
            Self::Unknown => "?",
        }
    }

    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Arbiter => "arbiter",
            Self::Mongos => "mongos",
            Self::Standalone => "standalone",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

/// One operation reported by `$currentOp`.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub key: OpKey,
    pub opid: OpId,
    pub source: OpSource,
    /// Server that reported the operation (`host` field).
    pub host: Option<String>,
    /// `shard` field (through mongos), or the replica set name / `"config"`
    /// of the polled node (`--all-nodes`).
    pub shard: Option<String>,
    /// Role of the polled node (`--all-nodes`), `None` otherwise.
    pub node_role: Option<NodeRole>,
    /// `type` field (`op`, `idleSession`, ...).
    pub op_type: String,
    /// `op` field (`query`, `update`, `command`, ...).
    pub op: String,
    pub ns: String,
    pub desc: String,
    pub secs_running: i64,
    pub microsecs_running: Option<i64>,
    /// `client` or `client_s`, empty when absent.
    pub client: String,
    /// `clientMetadata.mongos.host`: the mongos that forwarded the operation.
    pub mongos_host: Option<String>,
    pub app_name: Option<String>,
    pub effective_users: Vec<String>,
    pub plan_summary: Option<String>,
    /// `currentOpTime`: the start time of the operation. Together with the
    /// opid and host it identifies an operation, since opids are reused.
    pub current_op_time: Option<String>,
    pub kill_pending: bool,
    /// The full `$currentOp` document.
    pub raw: Document,
}

impl Operation {
    /// Running time in microseconds, for ordering.
    pub fn running_micros(&self) -> i64 {
        self.microsecs_running
            .unwrap_or_else(|| self.secs_running.saturating_mul(1_000_000))
    }

    /// Client address, followed by the short name of the forwarding mongos.
    pub fn client_display(&self) -> String {
        let client = if self.client.is_empty() {
            "N/A"
        } else {
            &self.client
        };
        match self.mongos_host.as_deref().filter(|h| !h.is_empty()) {
            Some(mongos) => {
                let short = mongos.split('.').next().unwrap_or(mongos);
                format!("{client} ({short})")
            }
            None => client.to_owned(),
        }
    }

    /// Comma-separated effective users, or `"N/A"`.
    pub fn users_display(&self) -> String {
        let users: Vec<&str> = self
            .effective_users
            .iter()
            .map(String::as_str)
            .filter(|u| !u.is_empty())
            .collect();
        if users.is_empty() {
            "N/A".to_owned()
        } else {
            users.join(", ")
        }
    }
}

/// Text filters from the filter bar. Every non-empty value is matched as a
/// case-insensitive substring; `running_time` is a minimum in seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub opid: String,
    pub operation: String,
    pub running_time: String,
    pub client: String,
    pub description: String,
    pub effective_users: String,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        [
            &self.opid,
            &self.operation,
            &self.running_time,
            &self.client,
            &self.description,
            &self.effective_users,
        ]
        .iter()
        .all(|v| v.trim().is_empty())
    }
}

/// What to fetch on a refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchQuery {
    pub filters: Filters,
    /// Also list the connected mongos' own operations (`localOps: true`).
    /// Ignored unless connected to a sharded cluster.
    pub include_mongos_local: bool,
}

/// Result of a refresh.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Operations ordered by running time, longest first, at most
    /// [`MAX_OPERATIONS`].
    pub operations: Vec<Operation>,
    /// More than [`MAX_OPERATIONS`] operations matched.
    pub truncated: bool,
    /// Polling status of each cluster member (`--all-nodes` only).
    pub nodes: Vec<NodeStatus>,
    /// Non-fatal problems to surface to the user.
    pub warnings: Vec<String>,
}

/// Polling status of one cluster member (`--all-nodes`).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeStatus {
    pub address: String,
    /// Shard / replica set name, `"config"` for config servers.
    pub shard: Option<String>,
    pub role: NodeRole,
    pub health: NodeHealth,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeHealth {
    /// Polled successfully.
    Ok {
        operations: usize,
        latency: Duration,
    },
    /// Could not be polled directly; the shard primary's operations are
    /// listed through mongos instead.
    Fallback { error: String },
    /// Could not be polled.
    Failed { error: String },
}

impl NodeHealth {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }
}

/// Kind of deployment behind the main connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deployment {
    Standalone,
    ReplicaSet {
        name: String,
    },
    /// Connected to mongos.
    Sharded,
}

/// Information about the established connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// Display name of the connection target (`host:port`, SRV name, ...).
    pub target: String,
    pub version: String,
    pub deployment: Deployment,
    pub load_balanced: bool,
    /// Every cluster member is polled directly (`--all-nodes`).
    pub all_nodes: bool,
}

impl ServerInfo {
    /// Short description, e.g. `"mongos, MongoDB 8.0.4, all nodes"`.
    pub fn describe(&self) -> String {
        let kind = match &self.deployment {
            Deployment::Standalone => "standalone".to_owned(),
            Deployment::ReplicaSet { name } => format!("replica set {name}"),
            Deployment::Sharded if self.load_balanced => "mongos via load balancer".to_owned(),
            Deployment::Sharded => "mongos".to_owned(),
        };
        let mut parts = vec![kind, format!("MongoDB {}", self.version)];
        if self.all_nodes {
            parts.push("all nodes".to_owned());
        }
        parts.join(", ")
    }

    pub fn is_sharded(&self) -> bool {
        self.deployment == Deployment::Sharded
    }
}

/// A request to kill one operation.
#[derive(Debug, Clone, PartialEq)]
pub struct KillRequest {
    pub key: OpKey,
    pub opid: OpId,
    pub source: OpSource,
    /// `host` of the operation when it was listed.
    pub host: Option<String>,
    /// `currentOpTime` of the operation when it was listed.
    pub current_op_time: Option<String>,
    /// Connection running the operation when it was listed (`desc`, e.g.
    /// `"conn123"`). With the start time, identifies the operation.
    pub connection: Option<String>,
    /// Summary for the log (namespace, op, client, command).
    pub description: String,
}

impl KillRequest {
    pub fn from_operation(op: &Operation) -> Self {
        let command = op
            .raw
            .get_document("command")
            .map(|c| {
                let mut summary = command_shape(c).to_string();
                if summary.len() > 500 {
                    let mut end = 500;
                    while !summary.is_char_boundary(end) {
                        end -= 1;
                    }
                    summary.truncate(end);
                    summary.push('…');
                }
                summary
            })
            .unwrap_or_default();
        Self {
            key: op.key.clone(),
            opid: op.opid.clone(),
            source: op.source.clone(),
            host: op.host.clone(),
            current_op_time: op.current_op_time.clone(),
            connection: (!op.desc.is_empty()).then(|| op.desc.clone()),
            description: format!(
                "ns={} op={} client={} command={command}",
                op.ns,
                op.op,
                op.client_display()
            ),
        }
    }
}

/// The command with every value replaced by "?", except the first field's
/// when it is a string (the command's target, e.g. "find": "users"): the log
/// records what ran, not other users' data. $currentOp lists updates and
/// deletes as their statement ({q, u, ...}), whose first field is the filter
/// document: it is redacted too.
fn command_shape(command: &Document) -> Document {
    fn redact(value: &Bson) -> Bson {
        match value {
            Bson::Document(d) => {
                Bson::Document(d.iter().map(|(k, v)| (k.clone(), redact(v))).collect())
            }
            Bson::Array(a) => Bson::Array(a.iter().map(redact).collect()),
            _ => Bson::String("?".to_owned()),
        }
    }
    command
        .iter()
        .enumerate()
        .map(|(i, (k, v))| {
            let keep = i == 0 && matches!(v, Bson::String(_));
            (k.clone(), if keep { v.clone() } else { redact(v) })
        })
        .collect()
}

/// Result of a kill request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillOutcome {
    /// killOp succeeded and the operation is gone.
    Killed,
    /// The operation finished before the kill was sent.
    AlreadyFinished,
    /// killOp was accepted but the operation is still running (e.g. it has
    /// `killPending` set and has not reached an interrupt point yet).
    StillRunning,
    /// The kill was not sent because it could hit a different operation.
    Refused(String),
    /// killOp failed.
    Failed(String),
}

impl KillOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Killed | Self::AlreadyFinished)
    }
}

impl fmt::Display for KillOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Killed => f.write_str("killed"),
            Self::AlreadyFinished => f.write_str("already finished"),
            Self::StillRunning => f.write_str("still running after the kill (kill pending)"),
            Self::Refused(reason) => write!(f, "not killed: {reason}"),
            Self::Failed(error) => write!(f, "kill failed: {error}"),
        }
    }
}

/// Results of a batch of kill requests, in request order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KillReport {
    pub results: Vec<(KillRequest, KillOutcome)>,
}

impl KillReport {
    pub fn succeeded(&self) -> usize {
        self.results.iter().filter(|(_, o)| o.is_success()).count()
    }

    pub fn failed(&self) -> usize {
        self.results.len() - self.succeeded()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sample_operation;

    #[test]
    fn opid_from_bson() {
        assert_eq!(OpId::from_bson(&Bson::Int32(7)), Some(OpId::Num(7)));
        assert_eq!(
            OpId::from_bson(&Bson::Int64(1 << 40)),
            Some(OpId::Num(1 << 40))
        );
        assert_eq!(OpId::from_bson(&Bson::Double(9.0)), Some(OpId::Num(9)));
        assert_eq!(OpId::from_bson(&Bson::Double(9.5)), None);
        assert_eq!(
            OpId::from_bson(&Bson::String(" shard01:42 ".into())),
            Some(OpId::Str("shard01:42".into()))
        );
        assert_eq!(OpId::from_bson(&Bson::String("  ".into())), None);
        assert_eq!(OpId::from_bson(&Bson::Null), None);
    }

    #[test]
    fn opid_to_bson_keeps_small_ids_as_int32() {
        assert_eq!(OpId::Num(5).to_bson(), Bson::Int32(5));
        assert_eq!(OpId::Num(1 << 40).to_bson(), Bson::Int64(1 << 40));
        assert_eq!(
            OpId::Str("s:1".into()).to_bson(),
            Bson::String("s:1".into())
        );
    }

    #[test]
    fn opid_shard_parts() {
        assert_eq!(
            OpId::Str("shard01:42".into()).shard_parts(),
            Some(("shard01", 42))
        );
        assert_eq!(
            OpId::Str("rs-a:b:42".into()).shard_parts(),
            Some(("rs-a:b", 42))
        );
        assert_eq!(OpId::Str(":42".into()).shard_parts(), None);
        assert_eq!(OpId::Str("shard01:x".into()).shard_parts(), None);
        assert_eq!(OpId::Num(42).shard_parts(), None);
    }

    #[test]
    fn op_keys_are_unique_per_source() {
        let id = OpId::Num(12);
        assert_eq!(OpKey::new(&OpSource::Main, &id, None).0, "12");
        assert_eq!(
            OpKey::new(&OpSource::MongosLocal, &id, Some("router:27017")).0,
            "mongos/router:27017/12"
        );
        assert_eq!(
            OpKey::new(
                &OpSource::Node {
                    address: "db2:27018".into()
                },
                &id,
                Some("db2:27018")
            )
            .0,
            "db2:27018/12"
        );
    }

    #[test]
    fn client_display_appends_short_mongos_name() {
        let mut op = sample_operation(OpId::Num(1));
        assert_eq!(op.client_display(), "10.0.0.1:5000");
        op.mongos_host = Some("router-1.example.com:27017".into());
        assert_eq!(op.client_display(), "10.0.0.1:5000 (router-1)");
        op.client.clear();
        assert_eq!(op.client_display(), "N/A (router-1)");
    }

    #[test]
    fn users_display() {
        let mut op = sample_operation(OpId::Num(1));
        op.effective_users = vec!["a".into(), String::new(), "b".into()];
        assert_eq!(op.users_display(), "a, b");
        op.effective_users.clear();
        assert_eq!(op.users_display(), "N/A");
    }

    #[test]
    fn running_micros_falls_back_to_seconds() {
        let mut op = sample_operation(OpId::Num(1));
        assert_eq!(op.running_micros(), 3_500_000);
        op.microsecs_running = None;
        assert_eq!(op.running_micros(), 3_000_000);
    }

    #[test]
    fn filters_is_empty_ignores_whitespace() {
        let mut f = Filters::default();
        assert!(f.is_empty());
        f.client = "  ".into();
        assert!(f.is_empty());
        f.client = "10.0".into();
        assert!(!f.is_empty());
    }

    #[test]
    fn server_info_describe() {
        let mut info = ServerInfo {
            target: "localhost:27017".into(),
            version: "8.0.4".into(),
            deployment: Deployment::Sharded,
            load_balanced: false,
            all_nodes: true,
        };
        assert_eq!(info.describe(), "mongos, MongoDB 8.0.4, all nodes");
        info.deployment = Deployment::ReplicaSet { name: "rs0".into() };
        info.all_nodes = false;
        assert_eq!(info.describe(), "replica set rs0, MongoDB 8.0.4");
    }

    #[test]
    fn kill_request_summarizes_operation() {
        let op = sample_operation(OpId::Num(9));
        let req = KillRequest::from_operation(&op);
        assert_eq!(req.key, op.key);
        assert_eq!(req.connection.as_deref(), Some("conn42"));
        assert!(req.description.contains("ns=app.users"));
        assert!(req.description.contains("\"find\": \"users\""));
        let mut anonymous = op.clone();
        anonymous.desc.clear();
        assert_eq!(KillRequest::from_operation(&anonymous).connection, None);
    }

    #[test]
    fn kill_request_logs_the_command_shape_not_its_values() {
        let mut op = sample_operation(OpId::Num(9));
        let req = KillRequest::from_operation(&op);
        assert!(req.description.contains("\"filter\": { \"age\": \"?\" }"));
        assert!(!req.description.contains('3'));

        // Updates are listed as their statement: the filter comes first.
        op.raw = mongodb::bson::doc! {
            "command": { "q": { "token": "s3cret" }, "u": { "$set": { "pw": "hash" } } }
        };
        let req = KillRequest::from_operation(&op);
        assert!(req.description.contains("\"q\""));
        assert!(req.description.contains("\"$set\""));
        assert!(!req.description.contains("s3cret"));
        assert!(!req.description.contains("hash"));
    }

    #[test]
    fn kill_report_counts() {
        let op = sample_operation(OpId::Num(9));
        let req = KillRequest::from_operation(&op);
        let report = KillReport {
            results: vec![
                (req.clone(), KillOutcome::Killed),
                (req.clone(), KillOutcome::AlreadyFinished),
                (req.clone(), KillOutcome::StillRunning),
                (req, KillOutcome::Failed("x".into())),
            ],
        };
        assert_eq!(report.succeeded(), 2);
        assert_eq!(report.failed(), 2);
    }
}
