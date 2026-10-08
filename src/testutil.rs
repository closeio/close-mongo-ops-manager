//! Helpers shared by unit tests.

use mongodb::bson::doc;

use crate::model::{OpId, OpKey, OpSource, Operation};

/// A plain `find` operation listed through the main connection.
pub(crate) fn sample_operation(opid: OpId) -> Operation {
    Operation {
        key: OpKey::new(&OpSource::Main, &opid, None),
        opid,
        source: OpSource::Main,
        host: Some("db1:27017".into()),
        shard: None,
        node_role: None,
        op_type: "op".into(),
        op: "query".into(),
        ns: "app.users".into(),
        desc: "conn42".into(),
        secs_running: 3,
        microsecs_running: Some(3_500_000),
        client: "10.0.0.1:5000".into(),
        mongos_host: None,
        app_name: None,
        effective_users: vec!["alice".into()],
        plan_summary: None,
        current_op_time: Some("2026-10-08T10:00:00.000+00:00".into()),
        kill_pending: false,
        raw: doc! { "command": { "find": "users", "filter": { "age": 3 } } },
    }
}

/// An operation with the given numeric opid and running time in seconds.
pub(crate) fn operation_running(opid: i64, secs: i64) -> Operation {
    let mut op = sample_operation(OpId::Num(opid));
    op.secs_running = secs;
    op.microsecs_running = Some(secs * 1_000_000);
    op
}
