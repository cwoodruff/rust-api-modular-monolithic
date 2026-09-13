//! Data-access failures and the responses they produce.
//!
//! [`RepositoryError`] lives here rather than beside the repository traits so
//! that this crate can turn one into a response without depending on the crate
//! that owns the traits. `shared-persistence` re-exports it, so callers there
//! see it where they expect to.

use axum::Json;
use axum::response::{IntoResponse, Response};

use crate::ProblemDetails;
use crate::errors::{ApiError, current_trace_id};

/// Something went wrong talking to the database.
///
/// The driver's own error type is erased because neither this crate nor the
/// contracts crate may depend on a driver. The C# equivalent is an unhandled
/// exception, which the host turns into a 500.
///
/// The cause is held behind an `Arc` rather than a `Box` so this is `Clone`.
/// The cache facade needs that: when several callers coalesce onto one factory
/// call and it fails, every one of them has to be handed the failure, and a
/// boxed error can only be handed to one.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RepositoryError {
    /// The underlying database reported a failure.
    #[error("database operation failed")]
    Database(#[source] std::sync::Arc<dyn std::error::Error + Send + Sync>),
}

impl RepositoryError {
    /// Wraps a driver error.
    pub fn database(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Database(std::sync::Arc::from(source.into()))
    }

    /// The 500 the host returns, leaking nothing about the cause.
    #[must_use]
    pub fn into_problem(self, trace_id: impl Into<String>) -> ProblemDetails {
        tracing::error!(error = %self, "repository call failed");

        ProblemDetails::internal_server_error(trace_id)
    }
}

impl IntoResponse for RepositoryError {
    fn into_response(self) -> Response {
        self.into_problem(current_trace_id()).into_response()
    }
}

/// Result of a repository call.
pub type RepositoryResult<T> = Result<T, RepositoryError>;

/// Renders a by-id lookup.
///
/// Port of `album is not null ? TypedResults.Ok(album) : Results.NotFound()`.
/// The 404 carries the document the status-code-pages layer would have written
/// for a bodiless one, so the wire output is what it always was.
///
/// # Errors
///
/// [`ApiError::NotFound`] when the lookup found nothing.
pub fn found<T>(value: Option<T>) -> Result<Json<T>, ApiError> {
    value.map(Json).ok_or(ApiError::NotFound)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_repository_error_reports_a_500_without_detail() {
        let problem = RepositoryError::database("no such table: Album").into_problem("trace-1");

        assert_eq!(problem.status, 500);
        assert_eq!(
            problem.detail, None,
            "the driver's message must not reach the client"
        );
    }

    #[test]
    fn the_driver_error_is_still_available_for_logging() {
        let error = RepositoryError::database("no such table: Album");

        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("no such table: Album".to_owned())
        );
    }

    #[test]
    fn a_found_item_is_serialized_and_a_missing_one_is_the_originals_404() {
        let present = found(Some(serde_json::json!({ "Id": 1 })))
            .expect("a present value should not be an error")
            .into_response();
        assert_eq!(present.status(), axum::http::StatusCode::OK);

        let missing = found::<serde_json::Value>(None)
            .expect_err("a missing value should be a 404")
            .into_problem("trace-1");

        assert_eq!(missing.status, 404);
        assert_eq!(missing.title, "Not Found");
    }

    #[test]
    fn a_repository_failure_becomes_the_bodiless_500() {
        let problem = ApiError::from(RepositoryError::database("boom")).into_problem("trace-1");

        assert_eq!(problem.status, 500);
        assert_eq!(problem.detail, None);
    }
}
