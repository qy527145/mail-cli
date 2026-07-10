use serde::Serialize;

/// Machine-readable error output.
///
/// Emitted on stdout when the CLI fails, so agents can parse the failure
/// without scraping stderr. Exit code carries the same signal for scripts.
#[derive(Debug, Serialize)]
pub struct ErrorOutput {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub kind: String,
    pub message: String,
    pub exit_code: i32,
}
