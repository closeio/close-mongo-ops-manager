//! `$currentOp` aggregation pipelines.

use mongodb::bson::{Bson, Document, doc};

use super::call::own_marker;
use crate::model::{Filters, MAX_OPERATIONS, OpId};

/// What to list.
#[derive(Debug, Clone)]
pub(crate) struct ListingSpec<'a> {
    pub filters: &'a Filters,
    /// Only list operations whose namespace starts with this.
    pub namespace: &'a str,
    pub hide_system_ops: bool,
    /// `localOps`: through mongos, list the mongos' own operations instead of
    /// the shards'.
    pub local_ops: bool,
    /// An extra condition, e.g. `{shard: {$in: [...]}}`.
    pub extra_match: Option<Document>,
}

/// Documents per batch when listing: the whole listing in one round trip.
pub(crate) const LISTING_BATCH_SIZE: u32 = MAX_OPERATIONS as u32 + 1;

/// Fields kept from each `$currentOp` document.
const PROJECTED_FIELDS: &[&str] = &[
    "opid",
    "host",
    "shard",
    "type",
    "op",
    "secs_running",
    "microsecs_running",
    "client",
    "client_s",
    "appName",
    "clientMetadata.mongos",
    "clientMetadata.application",
    "desc",
    "effectiveUsers",
    "active",
    "ns",
    "command",
    "originatingCommand",
    "planSummary",
    "currentOpTime",
    "killPending",
    "numYields",
    "msg",
    "progress",
    "connectionId",
    "lsid",
    "transaction",
    "locks",
    "waitingForLock",
    "lockStats",
];

/// The `$currentOp` stage.
///
/// Idle connections, cursors and sessions are left out: they have no opid
/// (or are not running), so they can be neither displayed nor killed.
fn current_op_stage(local_ops: bool) -> Document {
    doc! {
        "$currentOp": {
            "allUsers": true,
            "idleConnections": false,
            "idleCursors": false,
            "idleSessions": false,
            "localOps": local_ops,
            "backtrace": false,
        }
    }
}

/// The pipeline listing operations: `$currentOp`, the filters, a projection,
/// and the slowest [`MAX_OPERATIONS`] + 1 operations (one more than kept, to
/// detect truncation).
pub(crate) fn listing_pipeline(spec: &ListingSpec<'_>) -> Vec<Document> {
    let mut pipeline = vec![current_op_stage(spec.local_ops)];
    let conditions = match_conditions(spec);
    if !conditions.is_empty() {
        pipeline.push(doc! { "$match": { "$and": conditions } });
    }
    let mut projection = Document::new();
    for field in PROJECTED_FIELDS {
        projection.insert(*field, 1);
    }
    pipeline.push(doc! { "$project": projection });
    pipeline.push(doc! { "$sort": { "microsecs_running": -1 } });
    pipeline.push(doc! { "$limit": limit_value(MAX_OPERATIONS + 1) });
    pipeline
}

fn limit_value(n: usize) -> Bson {
    i64::try_from(n).map_or(Bson::Int64(i64::MAX), |n| {
        i32::try_from(n).map_or(Bson::Int64(n), Bson::Int32)
    })
}

/// The pipeline looking up one operation before and after killing it.
pub(crate) fn lookup_pipeline(opid: &OpId, local_ops: bool) -> Vec<Document> {
    vec![
        current_op_stage(local_ops),
        doc! { "$match": { "opid": opid.to_bson() } },
        doc! {
            "$project": {
                "opid": 1,
                "host": 1,
                "currentOpTime": 1,
                "microsecs_running": 1,
                "desc": 1,
                "connectionId": 1,
                "killPending": 1,
            }
        },
        doc! { "$limit": 1 },
    ]
}

/// Case-insensitive regular expression condition on `field`.
fn regex_condition(field: &str, pattern: String) -> Document {
    let mut condition = Document::new();
    condition.insert(field, doc! { "$regex": pattern, "$options": "i" });
    condition
}

fn match_conditions(spec: &ListingSpec<'_>) -> Vec<Document> {
    let mut conditions = Vec::new();

    if spec.hide_system_ops {
        // Namespaces of the admin/config/local databases only identify
        // internal work when no authenticated user other than the internal
        // __system@local runs it: any client picks the database of its own
        // database-level commands (e.g. `aggregate: 1` on admin or config).
        let internal_only = doc! {
            "effectiveUsers": { "$not": { "$elemMatch": {
                "$nor": [ { "user": "__system", "db": "local" } ]
            } } }
        };
        conditions.push(doc! {
            "$nor": [
                { "$and": [
                    { "$or": [
                        regex_condition("ns", "^admin\\.".to_owned()),
                        regex_condition("ns", "^config\\.".to_owned()),
                        regex_condition("ns", "^local\\.".to_owned()),
                    ] },
                    internal_only,
                ] },
                { "op": "none" },
                // The internal cluster user only: a user named __system in
                // any other database is a regular user.
                { "effectiveUsers": { "$elemMatch": { "user": "__system", "db": "local" } } },
                // Our own $currentOp aggregations carry a random per-process
                // marker as their comment. Never match on the application
                // name: any client can announce ours.
                { "command.comment": own_marker() },
            ]
        });
    }

    let namespace = spec.namespace.trim();
    if !namespace.is_empty() {
        conditions.push(regex_condition(
            "ns",
            format!("^{}", escape_regex(namespace)),
        ));
    }

    let filters = spec.filters;
    if let Some(opid) = non_empty(&filters.opid) {
        conditions.push(doc! {
            "$expr": {
                "$regexMatch": {
                    "input": { "$toString": "$opid" },
                    "regex": escape_regex(opid),
                    "options": "i",
                }
            }
        });
    }
    if let Some(operation) = non_empty(&filters.operation) {
        conditions.push(regex_condition("op", escape_regex(operation)));
    }
    if let Some(client) = non_empty(&filters.client) {
        let pattern = escape_regex(client);
        conditions.push(doc! {
            "$or": [
                regex_condition("client", pattern.clone()),
                regex_condition("client_s", pattern),
            ]
        });
    }
    if let Some(description) = non_empty(&filters.description) {
        conditions.push(regex_condition("desc", escape_regex(description)));
    }
    if let Some(users) = non_empty(&filters.effective_users) {
        conditions.push(doc! {
            "effectiveUsers": {
                "$elemMatch": regex_condition("user", escape_regex(users)),
            }
        });
    }
    if let Some(secs) = min_running_secs(&filters.running_time) {
        conditions.push(doc! { "secs_running": { "$gte": secs } });
    }

    if let Some(extra) = &spec.extra_match {
        conditions.push(extra.clone());
    }
    conditions
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

/// The running time filter: a non-negative number of seconds. Anything else
/// (including a sign) is ignored, as in the Python version.
fn min_running_secs(value: &str) -> Option<i64> {
    let value = non_empty(value)?;
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Only digits: parsing can only fail on overflow.
    Some(value.parse().unwrap_or(i64::MAX))
}

/// Escapes `text` so it matches literally in a PCRE regular expression:
/// every ASCII character other than letters, digits and `_` is preceded by a
/// backslash. Non-ASCII characters are kept as they are.
pub(crate) fn escape_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if c.is_ascii() && !c.is_ascii_alphanumeric() && c != '_' {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "close-mongo-ops-manager";

    fn spec(filters: &Filters) -> ListingSpec<'_> {
        ListingSpec {
            filters,
            namespace: "",
            hide_system_ops: false,
            local_ops: false,
            extra_match: None,
        }
    }

    fn conditions(pipeline: &[Document]) -> Vec<Document> {
        pipeline
            .iter()
            .find_map(|stage| stage.get_document("$match").ok())
            .map(|m| {
                m.get_array("$and")
                    .unwrap()
                    .iter()
                    .map(|c| c.as_document().unwrap().clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn stage_names(pipeline: &[Document]) -> Vec<&str> {
        pipeline
            .iter()
            .map(|s| s.keys().next().unwrap().as_str())
            .collect()
    }

    #[test]
    fn no_conditions_means_no_match_stage() {
        let filters = Filters::default();
        let pipeline = listing_pipeline(&spec(&filters));
        assert_eq!(
            stage_names(&pipeline),
            ["$currentOp", "$project", "$sort", "$limit"]
        );
    }

    #[test]
    fn current_op_stage_flags() {
        let filters = Filters::default();
        let pipeline = listing_pipeline(&spec(&filters));
        assert_eq!(
            pipeline[0],
            doc! {
                "$currentOp": {
                    "allUsers": true,
                    "idleConnections": false,
                    "idleCursors": false,
                    "idleSessions": false,
                    "localOps": false,
                    "backtrace": false,
                }
            }
        );
        let mut local = spec(&filters);
        local.local_ops = true;
        let pipeline = listing_pipeline(&local);
        let stage = pipeline[0].get_document("$currentOp").unwrap();
        assert_eq!(stage.get_bool("localOps"), Ok(true));
    }

    #[test]
    fn sorts_slowest_first_and_limits_one_past_max() {
        let filters = Filters::default();
        let pipeline = listing_pipeline(&spec(&filters));
        let n = pipeline.len();
        assert_eq!(
            pipeline[n - 2],
            doc! { "$sort": { "microsecs_running": -1 } }
        );
        assert_eq!(pipeline[n - 1], doc! { "$limit": 1001 });
    }

    #[test]
    fn projection_keeps_the_needed_fields() {
        let filters = Filters::default();
        let pipeline = listing_pipeline(&spec(&filters));
        let projection = pipeline
            .iter()
            .find_map(|s| s.get_document("$project").ok())
            .unwrap();
        for field in [
            "opid",
            "host",
            "shard",
            "secs_running",
            "microsecs_running",
            "client",
            "client_s",
            "appName",
            "clientMetadata.mongos",
            "clientMetadata.application",
            "effectiveUsers",
            "ns",
            "command",
            "planSummary",
            "currentOpTime",
            "killPending",
        ] {
            assert_eq!(projection.get_i32(field), Ok(1), "{field}");
        }
        assert!(!projection.contains_key("clientMetadata"));
    }

    #[test]
    fn system_filter_hides_internal_and_own_operations() {
        let filters = Filters::default();
        let mut s = spec(&filters);
        s.hide_system_ops = true;
        let conditions = conditions(&listing_pipeline(&s));
        assert_eq!(conditions.len(), 1);
        let nor = conditions[0].get_array("$nor").unwrap();
        let expected = vec![
            Bson::Document(doc! { "$and": [
                { "$or": [
                    { "ns": { "$regex": "^admin\\.", "$options": "i" } },
                    { "ns": { "$regex": "^config\\.", "$options": "i" } },
                    { "ns": { "$regex": "^local\\.", "$options": "i" } },
                ] },
                { "effectiveUsers": { "$not": { "$elemMatch": {
                    "$nor": [ { "user": "__system", "db": "local" } ]
                } } } },
            ] }),
            Bson::Document(doc! { "op": "none" }),
            Bson::Document(doc! {
                "effectiveUsers": { "$elemMatch": { "user": "__system", "db": "local" } }
            }),
            Bson::Document(doc! { "command.comment": own_marker() }),
        ];
        assert_eq!(nor, &expected);
    }

    #[test]
    fn system_filter_never_excludes_by_application_name() {
        // Any client can announce our application name.
        let filters = Filters::default();
        let mut s = spec(&filters);
        s.hide_system_ops = true;
        let pipeline = listing_pipeline(&s);
        let nor = conditions(&pipeline)[0].get_array("$nor").unwrap().clone();
        for condition in &nor {
            let condition = condition.as_document().unwrap();
            assert!(!condition.contains_key("appName"), "{condition}");
            assert!(
                !condition.contains_key("clientMetadata.application.name"),
                "{condition}"
            );
            assert!(
                !condition.values().any(|v| v.as_str() == Some(APP)),
                "{condition}"
            );
        }
    }

    #[test]
    fn namespace_is_an_escaped_prefix() {
        let filters = Filters::default();
        let mut s = spec(&filters);
        s.namespace = " app.users ";
        let conditions = conditions(&listing_pipeline(&s));
        assert_eq!(
            conditions,
            [doc! { "ns": { "$regex": "^app\\.users", "$options": "i" } }]
        );
    }

    #[test]
    fn opid_filter_matches_the_string_form() {
        let filters = Filters {
            opid: "shard01:12".into(),
            ..Filters::default()
        };
        let conditions = conditions(&listing_pipeline(&spec(&filters)));
        assert_eq!(
            conditions,
            [doc! {
                "$expr": {
                    "$regexMatch": {
                        "input": { "$toString": "$opid" },
                        "regex": "shard01\\:12",
                        "options": "i",
                    }
                }
            }]
        );
    }

    #[test]
    fn text_filters() {
        let filters = Filters {
            operation: "query".into(),
            client: "10.0.0.1".into(),
            description: "conn".into(),
            effective_users: "alice".into(),
            ..Filters::default()
        };
        let conditions = conditions(&listing_pipeline(&spec(&filters)));
        assert_eq!(
            conditions,
            [
                doc! { "op": { "$regex": "query", "$options": "i" } },
                doc! {
                    "$or": [
                        { "client": { "$regex": "10\\.0\\.0\\.1", "$options": "i" } },
                        { "client_s": { "$regex": "10\\.0\\.0\\.1", "$options": "i" } },
                    ]
                },
                doc! { "desc": { "$regex": "conn", "$options": "i" } },
                doc! {
                    "effectiveUsers": {
                        "$elemMatch": { "user": { "$regex": "alice", "$options": "i" } }
                    }
                },
            ]
        );
    }

    #[test]
    fn blank_filters_are_ignored() {
        let filters = Filters {
            opid: "  ".into(),
            operation: "\t".into(),
            client: " ".into(),
            description: String::new(),
            effective_users: " ".into(),
            running_time: " ".into(),
        };
        assert!(conditions(&listing_pipeline(&spec(&filters))).is_empty());
    }

    #[test]
    fn filter_values_are_trimmed() {
        let filters = Filters {
            operation: "  update ".into(),
            ..Filters::default()
        };
        let conditions = conditions(&listing_pipeline(&spec(&filters)));
        assert_eq!(
            conditions,
            [doc! { "op": { "$regex": "update", "$options": "i" } }]
        );
    }

    #[test]
    fn running_time_filter() {
        let running = |value: &str| {
            let filters = Filters {
                running_time: value.into(),
                ..Filters::default()
            };
            conditions(&listing_pipeline(&spec(&filters)))
        };
        assert_eq!(running("10"), [doc! { "secs_running": { "$gte": 10_i64 } }]);
        assert_eq!(running(" 0 "), [doc! { "secs_running": { "$gte": 0_i64 } }]);
        assert!(running("abc").is_empty());
        assert!(running("-5").is_empty());
        assert!(running("+5").is_empty());
        assert!(running("1.5").is_empty());
        assert_eq!(
            running("99999999999999999999"),
            [doc! { "secs_running": { "$gte": i64::MAX } }]
        );
    }

    #[test]
    fn combined_filters_and_extra_match() {
        let filters = Filters {
            opid: "123".into(),
            operation: "query".into(),
            running_time: "10".into(),
            client: "192.168".into(),
            description: "conn".into(),
            effective_users: "testuser".into(),
        };
        let mut s = spec(&filters);
        s.hide_system_ops = true;
        s.namespace = "app";
        s.extra_match = Some(doc! { "shard": { "$in": ["shard01"] } });
        let conditions = conditions(&listing_pipeline(&s));
        let keys: Vec<&str> = conditions
            .iter()
            .map(|c| c.keys().next().unwrap().as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "$nor",
                "ns",
                "$expr",
                "op",
                "$or",
                "desc",
                "effectiveUsers",
                "secs_running",
                "shard"
            ]
        );
        assert_eq!(
            conditions.last().unwrap(),
            &doc! { "shard": { "$in": ["shard01"] } }
        );
    }

    #[test]
    fn lookup_pipeline_matches_one_opid() {
        let pipeline = lookup_pipeline(&OpId::Str("shard01:7".into()), false);
        assert_eq!(
            stage_names(&pipeline),
            ["$currentOp", "$match", "$project", "$limit"]
        );
        assert_eq!(pipeline[1], doc! { "$match": { "opid": "shard01:7" } });
        assert_eq!(pipeline[3], doc! { "$limit": 1 });
        assert_eq!(
            pipeline[2],
            doc! {
                "$project": {
                    "opid": 1,
                    "host": 1,
                    "currentOpTime": 1,
                    "microsecs_running": 1,
                    "desc": 1,
                    "connectionId": 1,
                    "killPending": 1,
                }
            }
        );
        let local = lookup_pipeline(&OpId::Num(7), true);
        assert_eq!(local[1], doc! { "$match": { "opid": 7 } });
        let stage = local[0].get_document("$currentOp").unwrap();
        assert_eq!(stage.get_bool("localOps"), Ok(true));
        assert_eq!(stage.get_bool("idleSessions"), Ok(false));
    }

    #[test]
    fn escape_regex_escapes_ascii_punctuation() {
        assert_eq!(escape_regex("abc_XYZ09"), "abc_XYZ09");
        assert_eq!(escape_regex("a.b*c"), "a\\.b\\*c");
        assert_eq!(
            escape_regex(r"^$.|?*+()[]{}\/"),
            r"\^\$\.\|\?\*\+\(\)\[\]\{\}\\\/"
        );
        assert_eq!(escape_regex("10.0.0.1:5000"), "10\\.0\\.0\\.1\\:5000");
        assert_eq!(escape_regex("a b-c"), "a\\ b\\-c");
        assert_eq!(escape_regex("café ñ"), "café\\ ñ");
        assert_eq!(escape_regex(""), "");
    }
}
