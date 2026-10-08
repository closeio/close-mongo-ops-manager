//! Conversion of `$currentOp` documents into [`Operation`]s.

use chrono::{DateTime, FixedOffset, SecondsFormat, TimeDelta};
use mongodb::bson::{Bson, Document};

use crate::model::{MAX_OPERATIONS, NodeRole, OpId, OpKey, OpSource, Operation};

/// Two start times computed from different `$currentOp` reports of the same
/// operation differ by a few milliseconds at most. A different operation that
/// reuses the opid (after a restart or a failover) started at another time.
const START_TIME_TOLERANCE: TimeDelta = TimeDelta::seconds(1);

/// Where a batch of documents was listed.
#[derive(Debug, Clone)]
pub(crate) struct OpContext<'a> {
    pub source: &'a OpSource,
    /// Shard / replica set name of the polled node; the document's own
    /// `shard` field takes precedence.
    pub shard: Option<&'a str>,
    pub node_role: Option<NodeRole>,
    /// Host to report when the document has no `host` field.
    pub default_host: Option<&'a str>,
}

/// Parses `$currentOp` documents, skipping (and logging) the ones without a
/// usable opid.
pub(crate) fn parse_operations(docs: Vec<Document>, ctx: &OpContext<'_>) -> Vec<Operation> {
    let total = docs.len();
    let ops: Vec<Operation> = docs
        .into_iter()
        .filter_map(|doc| parse_operation(doc, ctx))
        .collect();
    if ops.len() < total {
        log::debug!(
            "Skipped {} operations without an opid ({:?})",
            total - ops.len(),
            ctx.source
        );
    }
    ops
}

/// Parses one `$currentOp` document. `None` when it has no usable opid.
pub(crate) fn parse_operation(doc: Document, ctx: &OpContext<'_>) -> Option<Operation> {
    let opid = doc.get("opid").and_then(OpId::from_bson)?;
    let host = string(&doc, "host").or_else(|| ctx.default_host.map(str::to_owned));
    let shard = string(&doc, "shard").or_else(|| ctx.shard.map(str::to_owned));
    let client = string(&doc, "client")
        .or_else(|| string(&doc, "client_s"))
        .unwrap_or_default();
    let metadata = doc.get_document("clientMetadata").ok();
    let mongos_host = metadata
        .and_then(|m| m.get_document("mongos").ok())
        .and_then(|m| string(m, "host"));
    let app_name = string(&doc, "appName").or_else(|| {
        metadata
            .and_then(|m| m.get_document("application").ok())
            .and_then(|a| string(a, "name"))
    });
    let effective_users = doc
        .get_array("effectiveUsers")
        .map(|users| {
            users
                .iter()
                .filter_map(|u| u.as_document().and_then(|u| string(u, "user")))
                .collect()
        })
        .unwrap_or_default();

    Some(Operation {
        key: OpKey::new(ctx.source, &opid, host.as_deref()),
        opid,
        source: ctx.source.clone(),
        host,
        shard,
        node_role: ctx.node_role,
        op_type: string(&doc, "type").unwrap_or_default(),
        op: string(&doc, "op").unwrap_or_default(),
        ns: string(&doc, "ns").unwrap_or_default(),
        desc: string(&doc, "desc").unwrap_or_default(),
        secs_running: doc.get("secs_running").and_then(as_i64).unwrap_or(0),
        microsecs_running: doc.get("microsecs_running").and_then(as_i64),
        client,
        mongos_host,
        app_name,
        effective_users,
        plan_summary: string(&doc, "planSummary"),
        current_op_time: start_time(&doc),
        kill_pending: matches!(doc.get("killPending"), Some(Bson::Boolean(true))),
        raw: doc,
    })
}

/// A non-empty string field.
fn string(doc: &Document, key: &str) -> Option<String> {
    match doc.get(key) {
        Some(Bson::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// A number stored as Int32, Int64 or Double.
pub(crate) fn as_i64(value: &Bson) -> Option<i64> {
    match value {
        Bson::Int32(v) => Some(i64::from(*v)),
        Bson::Int64(v) => Some(*v),
        // Saturating conversion; the fraction is dropped.
        Bson::Double(v) if v.is_finite() => Some(*v as i64),
        _ => None,
    }
}

/// Start time of the operation (RFC 3339, millisecond precision).
///
/// `currentOpTime` is the time the `$currentOp` report was produced, not the
/// start of the operation, so it changes on every refresh. The start time,
/// `currentOpTime - microsecs_running`, is stable and, together with the opid
/// and host, identifies an operation.
pub(crate) fn start_time(doc: &Document) -> Option<String> {
    let reported = match doc.get("currentOpTime")? {
        Bson::String(s) => DateTime::parse_from_rfc3339(s).ok()?,
        Bson::DateTime(dt) => {
            DateTime::from_timestamp_millis(dt.timestamp_millis())?.fixed_offset()
        }
        _ => return None,
    };
    let running = doc.get("microsecs_running").and_then(as_i64)?;
    let start = reported.checked_sub_signed(TimeDelta::microseconds(running))?;
    Some(start.to_rfc3339_opts(SecondsFormat::Millis, false))
}

/// Whether two start times from [`start_time`] belong to the same operation.
pub(crate) fn same_start_time(a: &str, b: &str) -> bool {
    match (parse_time(a), parse_time(b)) {
        (Some(a), Some(b)) => (a - b).abs() <= START_TIME_TOLERANCE,
        _ => a == b,
    }
}

fn parse_time(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

/// Merges listings: longest running first, at most [`MAX_OPERATIONS`].
/// Returns whether operations were dropped (or `truncated` already).
pub(crate) fn merge_operations(
    lists: impl IntoIterator<Item = Vec<Operation>>,
    truncated: bool,
) -> (Vec<Operation>, bool) {
    let mut ops: Vec<Operation> = lists.into_iter().flatten().collect();
    ops.sort_by(|a, b| {
        b.running_micros()
            .cmp(&a.running_micros())
            .then_with(|| a.key.cmp(&b.key))
    });
    let dropped = ops.len() > MAX_OPERATIONS;
    ops.truncate(MAX_OPERATIONS);
    (ops, truncated || dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::operation_running;
    use mongodb::bson::doc;

    fn main_ctx() -> OpContext<'static> {
        OpContext {
            source: &OpSource::Main,
            shard: None,
            node_role: None,
            default_host: None,
        }
    }

    fn mongod_doc() -> Document {
        doc! {
            "type": "op",
            "host": "db-1.example.com:27017",
            "desc": "conn11007",
            "connectionId": 11007,
            "client": "10.11.17.243:41024",
            "appName": "worker",
            "clientMetadata": { "application": { "name": "worker" } },
            "active": true,
            "currentOpTime": "2025-11-01T05:37:31.358+00:00",
            "effectiveUsers": [{ "user": "closeio2", "db": "closeio" }, { "user": "ops", "db": "admin" }],
            "opid": 727852,
            "secs_running": 0_i64,
            "microsecs_running": 173436_i64,
            "op": "query",
            "ns": "closeio.activity",
            "command": { "find": "activity", "limit": 101 },
            "planSummary": "IXSCAN { organization: 1 }",
            "numYields": 21,
        }
    }

    #[test]
    fn parses_a_mongod_document() {
        let op = parse_operation(mongod_doc(), &main_ctx()).unwrap();
        assert_eq!(op.opid, OpId::Num(727852));
        assert_eq!(op.key, OpKey("727852".into()));
        assert_eq!(op.source, OpSource::Main);
        assert_eq!(op.host.as_deref(), Some("db-1.example.com:27017"));
        assert_eq!(op.shard, None);
        assert_eq!(op.node_role, None);
        assert_eq!(op.op_type, "op");
        assert_eq!(op.op, "query");
        assert_eq!(op.ns, "closeio.activity");
        assert_eq!(op.desc, "conn11007");
        assert_eq!(op.secs_running, 0);
        assert_eq!(op.microsecs_running, Some(173436));
        assert_eq!(op.client, "10.11.17.243:41024");
        assert_eq!(op.mongos_host, None);
        assert_eq!(op.app_name.as_deref(), Some("worker"));
        assert_eq!(op.effective_users, ["closeio2", "ops"]);
        assert_eq!(
            op.plan_summary.as_deref(),
            Some("IXSCAN { organization: 1 }")
        );
        assert_eq!(
            op.current_op_time.as_deref(),
            Some("2025-11-01T05:37:31.184+00:00")
        );
        assert!(!op.kill_pending);
        assert_eq!(op.raw, mongod_doc());
    }

    #[test]
    fn parses_a_mongos_document() {
        let doc = doc! {
            "shard": "shard02",
            "type": "op",
            "host": "localhost:37024",
            "desc": "conn44",
            "client_s": "127.0.0.1:35474",
            "clientMetadata": {
                "application": { "name": "loader" },
                "mongos": { "host": "router-1.example.com:27017", "client": "10.0.0.9:5000" },
            },
            "opid": "shard02:99333",
            "secs_running": 2_i64,
            "microsecs_running": 2_082_930_i64,
            "op": "query",
            "ns": "cmomit.load",
            "killPending": true,
        };
        let op = parse_operation(doc, &main_ctx()).unwrap();
        assert_eq!(op.opid, OpId::Str("shard02:99333".into()));
        assert_eq!(op.key, OpKey("shard02:99333".into()));
        assert_eq!(op.shard.as_deref(), Some("shard02"));
        assert_eq!(op.client, "127.0.0.1:35474");
        assert_eq!(
            op.mongos_host.as_deref(),
            Some("router-1.example.com:27017")
        );
        assert_eq!(op.app_name.as_deref(), Some("loader"));
        assert!(op.kill_pending);
        assert_eq!(op.current_op_time, None);
    }

    #[test]
    fn node_context_fills_shard_role_and_host() {
        let source = OpSource::Node {
            address: "db-2:27018".into(),
        };
        let ctx = OpContext {
            source: &source,
            shard: Some("shard01"),
            node_role: Some(NodeRole::Secondary),
            default_host: Some("db-2:27018"),
        };
        let op = parse_operation(doc! { "opid": 5, "op": "query" }, &ctx).unwrap();
        assert_eq!(op.key, OpKey("db-2:27018/5".into()));
        assert_eq!(op.host.as_deref(), Some("db-2:27018"));
        assert_eq!(op.shard.as_deref(), Some("shard01"));
        assert_eq!(op.node_role, Some(NodeRole::Secondary));
        assert_eq!(op.source, source);
    }

    #[test]
    fn mongos_local_key_uses_the_mongos_host() {
        let ctx = OpContext {
            source: &OpSource::MongosLocal,
            shard: None,
            node_role: None,
            default_host: None,
        };
        let op = parse_operation(doc! { "opid": 53251, "host": "router:27017" }, &ctx).unwrap();
        assert_eq!(op.key, OpKey("mongos/router:27017/53251".into()));
    }

    #[test]
    fn skips_documents_without_an_opid() {
        let docs = vec![
            doc! { "op": "query", "ns": "test.collection" },
            doc! { "opid": Bson::Null, "op": "query" },
            doc! { "opid": "  ", "op": "query" },
            doc! { "opid": 12345, "op": "update", "ns": "test.users" },
        ];
        let ops = parse_operations(docs, &main_ctx());
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].opid, OpId::Num(12345));
    }

    #[test]
    fn null_and_missing_fields_get_defaults() {
        let doc = doc! {
            "opid": 12345,
            "op": Bson::Null,
            "ns": Bson::Null,
            "client": Bson::Null,
            "desc": Bson::Null,
            "secs_running": Bson::Null,
            "effectiveUsers": Bson::Null,
            "command": Bson::Null,
            "clientMetadata": Bson::Null,
            "appName": "",
        };
        let op = parse_operation(doc, &main_ctx()).unwrap();
        assert_eq!(op.op, "");
        assert_eq!(op.ns, "");
        assert_eq!(op.client, "");
        assert_eq!(op.desc, "");
        assert_eq!(op.secs_running, 0);
        assert_eq!(op.microsecs_running, None);
        assert!(op.effective_users.is_empty());
        assert_eq!(op.app_name, None);
        assert_eq!(op.host, None);
        assert_eq!(op.current_op_time, None);
    }

    #[test]
    fn numbers_of_any_type() {
        for (value, expected) in [
            (Bson::Int32(7), Some(7)),
            (Bson::Int64(1 << 40), Some(1 << 40)),
            (Bson::Double(3.9), Some(3)),
            (Bson::Double(f64::NAN), None),
            (Bson::String("3".into()), None),
            (Bson::Null, None),
        ] {
            assert_eq!(as_i64(&value), expected, "{value:?}");
        }
        let doc = doc! { "opid": 1, "secs_running": 4.0, "microsecs_running": 4_500_000 };
        let op = parse_operation(doc, &main_ctx()).unwrap();
        assert_eq!(op.secs_running, 4);
        assert_eq!(op.microsecs_running, Some(4_500_000));
    }

    #[test]
    fn start_time_subtracts_the_running_time() {
        let doc = doc! {
            "currentOpTime": "2026-10-08T16:29:13.122+00:00",
            "microsecs_running": 2_020_640_i64,
        };
        assert_eq!(
            start_time(&doc).as_deref(),
            Some("2026-10-08T16:29:11.101+00:00")
        );
        // The offset of the server is kept.
        let doc = doc! {
            "currentOpTime": "2026-10-08T18:29:13.122+02:00",
            "microsecs_running": 1_000,
        };
        assert_eq!(
            start_time(&doc).as_deref(),
            Some("2026-10-08T18:29:13.121+02:00")
        );
        // A BSON date is accepted too.
        let date = mongodb::bson::DateTime::from_millis(1_791_476_953_122);
        let doc = doc! { "currentOpTime": date, "microsecs_running": 0 };
        assert_eq!(
            start_time(&doc).as_deref(),
            Some("2026-10-08T16:29:13.122+00:00")
        );
        assert_eq!(
            start_time(&doc! { "currentOpTime": "2026-10-08T16:29:13.122+00:00" }),
            None
        );
        assert_eq!(start_time(&doc! { "microsecs_running": 5 }), None);
        assert_eq!(
            start_time(&doc! { "currentOpTime": "yesterday", "microsecs_running": 5 }),
            None
        );
    }

    #[test]
    fn start_time_is_stable_across_reports() {
        // Two reports of the same operation, 1.5 s apart.
        let first = start_time(&doc! {
            "currentOpTime": "2026-10-08T16:29:13.122+00:00",
            "microsecs_running": 2_020_640_i64,
        })
        .unwrap();
        let second = start_time(&doc! {
            "currentOpTime": "2026-10-08T16:29:14.624+00:00",
            "microsecs_running": 3_523_080_i64,
        })
        .unwrap();
        assert!(same_start_time(&first, &second));
    }

    #[test]
    fn same_start_time_tolerance() {
        assert!(same_start_time(
            "2026-10-08T16:29:11.101+00:00",
            "2026-10-08T18:29:11.900+02:00"
        ));
        assert!(!same_start_time(
            "2026-10-08T16:29:11.101+00:00",
            "2026-10-08T16:29:12.200+00:00"
        ));
        assert!(same_start_time("not a date", "not a date"));
        assert!(!same_start_time(
            "not a date",
            "2026-10-08T16:29:11.101+00:00"
        ));
    }

    #[test]
    fn merge_sorts_longest_first() {
        let lists = vec![
            vec![operation_running(1, 5), operation_running(2, 50)],
            vec![operation_running(3, 20)],
            vec![],
        ];
        let (ops, truncated) = merge_operations(lists, false);
        let ids: Vec<OpId> = ops.into_iter().map(|o| o.opid).collect();
        assert_eq!(ids, [OpId::Num(2), OpId::Num(3), OpId::Num(1)]);
        assert!(!truncated);
    }

    #[test]
    fn merge_breaks_ties_by_key() {
        let lists = vec![vec![operation_running(2, 5)], vec![operation_running(1, 5)]];
        let (ops, _) = merge_operations(lists, false);
        let ids: Vec<OpId> = ops.into_iter().map(|o| o.opid).collect();
        assert_eq!(ids, [OpId::Num(1), OpId::Num(2)]);
    }

    #[test]
    fn merge_keeps_the_slowest_operations() {
        let first: Vec<Operation> = (0..700).map(|i| operation_running(i, i)).collect();
        let second: Vec<Operation> = (700..1400).map(|i| operation_running(i, i)).collect();
        let (ops, truncated) = merge_operations([first, second], false);
        assert!(truncated);
        assert_eq!(ops.len(), MAX_OPERATIONS);
        assert_eq!(ops[0].opid, OpId::Num(1399));
        assert_eq!(ops[MAX_OPERATIONS - 1].opid, OpId::Num(400));
    }

    #[test]
    fn merge_keeps_an_earlier_truncation() {
        let (ops, truncated) = merge_operations([vec![operation_running(1, 1)]], true);
        assert_eq!(ops.len(), 1);
        assert!(truncated);
    }
}
