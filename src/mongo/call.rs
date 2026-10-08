//! Bounded server round trips and error descriptions.

use std::fmt;
use std::future::IntoFuture;
use std::hash::{BuildHasher, RandomState};
use std::sync::OnceLock;
use std::time::Duration;

use futures::TryStreamExt;
use mongodb::Client;
use mongodb::bson::{self, Bson, Document, RawDocumentBuf};
use mongodb::error::{Error, ErrorKind};

use super::APP_NAME;
use crate::error::MongoOpsError;

/// Extra time given to a round trip beyond the timeout before it is
/// abandoned. The driver's own server selection timeout is the same as ours
/// and its error says why no server was available (e.g. connection refused),
/// so it should win when both expire together.
const GRACE: Duration = Duration::from_millis(500);

/// Server error codes.
const AUTHENTICATION_FAILED: i32 = 18;
const BSON_OBJECT_TOO_LARGE: i32 = 10334;

/// A failed server round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallError {
    Timeout {
        what: String,
        after: Duration,
    },
    Driver {
        what: String,
        message: String,
        /// Authentication was rejected.
        auth_failed: bool,
    },
}

impl CallError {
    pub(crate) fn driver(what: &str, error: &Error) -> Self {
        let auth_failed = match error.kind.as_ref() {
            ErrorKind::Authentication { .. } => true,
            ErrorKind::Command(e) => e.code == AUTHENTICATION_FAILED,
            // Connections authenticate when they are established: when that
            // fails, server selection reports the server's error.
            ErrorKind::ServerSelection { message, .. } => mentions_auth_failure(message),
            _ => false,
        };
        Self::Driver {
            what: what.to_owned(),
            message: describe_error(error),
            auth_failed,
        }
    }

    pub(crate) fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            Self::Driver {
                auth_failed: true,
                ..
            }
        )
    }

    pub(crate) fn into_connection_error(self) -> MongoOpsError {
        match self {
            Self::Timeout { what, after } => MongoOpsError::Timeout { what, after },
            Self::Driver { .. } => MongoOpsError::Connection(self.to_string()),
        }
    }

    pub(crate) fn into_operation_error(self) -> MongoOpsError {
        match self {
            Self::Timeout { what, after } => MongoOpsError::Timeout { what, after },
            Self::Driver { .. } => MongoOpsError::Operation(self.to_string()),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { what, after } => {
                write!(f, "{what} timed out after {}s", after.as_secs_f64())
            }
            Self::Driver { what, message, .. } => write!(f, "{what}: {message}"),
        }
    }
}

/// A readable description of a driver error.
pub(crate) fn describe_error(error: &Error) -> String {
    let message = match error.kind.as_ref() {
        ErrorKind::Command(e) => {
            let hint = command_hint(e.code)
                .map(|h| format!(". {h}"))
                .unwrap_or_default();
            format!("{} ({}): {}{hint}", e.code_name, e.code, e.message)
        }
        ErrorKind::Authentication { message, .. } => format!("authentication failed: {message}"),
        kind => kind.to_string(),
    };
    tidy(&message)
}

/// What a server error means for this tool, when it is not obvious.
fn command_hint(code: i32) -> Option<&'static str> {
    match code {
        // $currentOp builds every operation's report before any filter, and
        // fails as a whole when one is larger than the server's limit.
        BSON_OBJECT_TOO_LARGE => Some(
            "An operation's $currentOp report is too large for the server to \
             return, so operations cannot be listed or killed safely on this \
             server until it ends (with --all-nodes, the other members are \
             still listed)",
        ),
        _ => None,
    }
}

fn mentions_auth_failure(message: &str) -> bool {
    message.contains("Authentication failed") || message.contains("AuthenticationFailed")
}

/// Removes the debugging details the driver includes in nested errors
/// (server selection errors list the error of every server).
fn tidy(message: &str) -> String {
    let mut tidy = remove_nested(message, ", labels: {", '{', '}');
    tidy = remove_nested(&tidy, ", source: Some(", '(', ')');
    tidy = remove_nested(&tidy, ", server response: Some(", '(', ')');
    tidy.replace(", source: None", "")
        .replace(", server response: None", "")
        .replace("Error: Kind: ", "Error: ")
}

/// Removes every occurrence of `prefix` (which ends with `open`) up to the
/// matching `close`.
fn remove_nested(text: &str, prefix: &str, open: char, close: char) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(prefix) {
        kept.push_str(&rest[..start]);
        let inner = &rest[start + prefix.len()..];
        let mut depth = 1_usize;
        let mut end = inner.len();
        for (i, c) in inner.char_indices() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    end = i + c.len_utf8();
                    break;
                }
            }
        }
        rest = &inner[end..];
    }
    kept.push_str(rest);
    kept
}

/// Runs `future`, giving up after `timeout` (plus a short grace period).
pub(crate) async fn bounded<T>(
    what: &str,
    timeout: Duration,
    future: impl IntoFuture<Output = mongodb::error::Result<T>>,
) -> Result<T, CallError> {
    match tokio::time::timeout(timeout + GRACE, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(CallError::driver(what, &error)),
        Err(_) => Err(CallError::Timeout {
            what: what.to_owned(),
            after: timeout,
        }),
    }
}

/// Runs a command on the `admin` database.
pub(crate) async fn admin_command(
    client: &Client,
    what: &str,
    command: Document,
    timeout: Duration,
) -> Result<Document, CallError> {
    let admin = client.database("admin");
    bounded(what, timeout, admin.run_command(command)).await
}

/// A random marker, fixed for the life of the process, sent as the comment
/// of our own aggregations so that the listing can hide them. Unlike the
/// application name, other clients cannot know it: it only shows up in
/// `$currentOp` to users allowed to see every user's operations.
pub(crate) fn own_marker() -> &'static str {
    static MARKER: OnceLock<String> = OnceLock::new();
    MARKER.get_or_init(|| {
        // RandomState is seeded from the operating system's random source.
        let word = || RandomState::new().hash_one(std::process::id());
        format!("{APP_NAME}-{:016x}{:016x}", word(), word())
    })
}

/// Runs an aggregation on the `admin` database and collects its documents.
/// The server is asked to give up after `timeout` too.
///
/// Documents are decoded one by one and leniently: `$currentOp` echoes other
/// clients' command values verbatim, so one invalid UTF-8 string must not
/// discard the whole batch. Invalid sequences become U+FFFD; a document that
/// still cannot be decoded is skipped with a warning.
pub(crate) async fn admin_aggregate(
    client: &Client,
    what: &str,
    pipeline: Vec<Document>,
    batch_size: u32,
    timeout: Duration,
) -> Result<Vec<Document>, CallError> {
    let admin = client.database("admin");
    let raw: Vec<RawDocumentBuf> = bounded(what, timeout, async {
        admin
            .aggregate(pipeline)
            .max_time(timeout)
            .batch_size(batch_size)
            .comment(Bson::String(own_marker().to_owned()))
            .await?
            .with_type::<RawDocumentBuf>()
            .try_collect()
            .await
    })
    .await?;
    Ok(decode_lossy(what, &raw))
}

fn decode_lossy(what: &str, raw: &[RawDocumentBuf]) -> Vec<Document> {
    let docs: Vec<Document> = raw
        .iter()
        .filter_map(|r| bson::from_slice_utf8_lossy::<Document>(r.as_bytes()).ok())
        .collect();
    if docs.len() < raw.len() {
        log::warn!(
            "{what}: skipped {} undecodable documents",
            raw.len() - docs.len()
        );
    }
    docs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{"s": <string with an invalid UTF-8 byte>}` as raw BSON.
    fn invalid_utf8_document() -> RawDocumentBuf {
        let value = b"ab\xffcd";
        let mut bytes = Vec::new();
        let len = 4 + 1 + 2 + 4 + value.len() + 1 + 1;
        bytes.extend_from_slice(&(len as i32).to_le_bytes());
        bytes.push(0x02);
        bytes.extend_from_slice(b"s\0");
        bytes.extend_from_slice(&((value.len() + 1) as i32).to_le_bytes());
        bytes.extend_from_slice(value);
        bytes.push(0);
        bytes.push(0);
        RawDocumentBuf::from_bytes(bytes).unwrap()
    }

    #[test]
    fn one_invalid_document_does_not_hide_the_others() {
        let good = RawDocumentBuf::from_document(&bson::doc! { "opid": 1 }).unwrap();
        let docs = decode_lossy("listing", &[good, invalid_utf8_document()]);
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].get_i32("opid").unwrap(), 1);
        assert_eq!(docs[1].get_str("s").unwrap(), "ab\u{fffd}cd");
    }

    #[test]
    fn own_marker_is_stable_and_unguessable() {
        let marker = own_marker();
        assert_eq!(marker, own_marker());
        let suffix = marker.strip_prefix("close-mongo-ops-manager-").unwrap();
        assert_eq!(suffix.len(), 32);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn timeout_errors_map_to_timeout() {
        let error = CallError::Timeout {
            what: "ping".into(),
            after: Duration::from_millis(1500),
        };
        assert_eq!(error.to_string(), "ping timed out after 1.5s");
        assert!(matches!(
            error.clone().into_connection_error(),
            MongoOpsError::Timeout { what, after } if what == "ping" && after == Duration::from_millis(1500)
        ));
        assert!(matches!(
            error.into_operation_error(),
            MongoOpsError::Timeout { .. }
        ));
    }

    #[test]
    fn driver_errors_keep_context() {
        let error = CallError::Driver {
            what: "listShards".into(),
            message: "Unauthorized (13): not authorized".into(),
            auth_failed: false,
        };
        assert_eq!(
            error.to_string(),
            "listShards: Unauthorized (13): not authorized"
        );
        assert!(!error.is_auth_failure());
        match error.clone().into_operation_error() {
            MongoOpsError::Operation(message) => {
                assert_eq!(message, "listShards: Unauthorized (13): not authorized")
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            error.into_connection_error(),
            MongoOpsError::Connection(_)
        ));
    }

    #[test]
    fn tidies_server_selection_errors() {
        let message = "Server selection timeout: No available servers. Topology: { Type: \
            Unknown, Servers: [ { Address: localhost:37039, Type: Unknown, Error: Kind: I/O \
            error: Connection refused (os error 61), labels: {\"RetryableError\", \
            \"SystemOverloadedError\"}, source: None, server response: None } ] }";
        assert_eq!(
            tidy(message),
            "Server selection timeout: No available servers. Topology: { Type: Unknown, \
             Servers: [ { Address: localhost:37039, Type: Unknown, Error: I/O error: \
             Connection refused (os error 61) } ] }"
        );
        assert_eq!(tidy("Kind: x, labels: {}"), "Kind: x");
        assert_eq!(tidy("broken, labels: {\"a\""), "broken");
        assert_eq!(tidy("plain"), "plain");
        let auth = "Server selection timeout: No available servers. Topology: { Type: Single, \
            Servers: [ { Address: db1:27018, Type: Unknown, Error: SCRAM failure: \
            Authentication failed., source: Some(Error { kind: Command(CommandError { code: \
            18, code_name: \"AuthenticationFailed\", message: \"Authentication failed.\", \
            topology_version: None, base_backoff: None }), wire_version: None, \
            server_response: None }) } ] }";
        assert_eq!(
            tidy(auth),
            "Server selection timeout: No available servers. Topology: { Type: Single, \
             Servers: [ { Address: db1:27018, Type: Unknown, Error: SCRAM failure: \
             Authentication failed. } ] }"
        );
        assert!(mentions_auth_failure(auth));
        assert!(!mentions_auth_failure("I/O error: Connection refused"));
    }

    #[test]
    fn oversized_reports_are_explained() {
        assert!(
            command_hint(BSON_OBJECT_TOO_LARGE)
                .unwrap()
                .contains("too large")
        );
        assert_eq!(command_hint(AUTHENTICATION_FAILED), None);
    }

    #[test]
    fn describes_driver_errors() {
        let error = Error::custom("boom");
        let message = describe_error(&error);
        assert!(message.starts_with("Custom user error"), "{message}");
        let call = CallError::driver("ping", &error);
        assert!(!call.is_auth_failure());
    }

    #[tokio::test]
    async fn bounded_times_out() {
        let result: Result<(), CallError> = bounded(
            "sleep",
            Duration::from_millis(10),
            std::future::pending::<mongodb::error::Result<()>>(),
        )
        .await;
        assert_eq!(
            result,
            Err(CallError::Timeout {
                what: "sleep".into(),
                after: Duration::from_millis(10)
            })
        );
    }

    #[tokio::test]
    async fn bounded_passes_results_through() {
        let ok = bounded("x", Duration::from_secs(1), async { Ok(5) }).await;
        assert_eq!(ok, Ok(5));
        let err = bounded("x", Duration::from_secs(1), async {
            Err::<(), _>(Error::custom(1))
        })
        .await;
        assert!(matches!(err, Err(CallError::Driver { what, .. }) if what == "x"));
    }
}
