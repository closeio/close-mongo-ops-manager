//! Error types.

use std::time::Duration;

/// Errors raised by the MongoDB layer.
#[derive(Debug, Clone, thiserror::Error)]
pub enum MongoOpsError {
    /// The connection could not be established.
    #[error("failed to connect to MongoDB: {0}")]
    Connection(String),
    /// Listing operations failed.
    #[error("failed to get operations: {0}")]
    Operation(String),
    /// A server round trip did not complete in time.
    #[error("{what} timed out after {}s", .after.as_secs_f64())]
    Timeout { what: String, after: Duration },
}
