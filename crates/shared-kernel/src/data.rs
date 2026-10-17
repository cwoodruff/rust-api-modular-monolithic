//! Data-access failures and the responses they produce.
//!
//! [`DataError`] is what a module's data access reports when the database
//! refuses. Upstream has no equivalent type: an EF Core exception propagates
//! and the host's exception handler turns it into a 500. Here it is a value,
//! and becomes the same 500.

use axum::Json;
use axum::response::{IntoResponse, Response};

use crate::ProblemDetails;
use crate::errors::{ApiError, current_trace_id};

/// Something went wrong talking to the database.
///
/// The driver's error is erased so the response path need not know the driver. The C# equivalent is an unhandled
/// exception, which the host turns into a 500.
///
/// The cause is held behind an `Arc` rather than a `Box` so this is `Clone`.
/// The cache facade needs that: when several callers coalesce onto one factory
/// call and it fails, every one of them has to be handed the failure, and a
/// boxed error can only be handed to one.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DataError {
    /// The underlying database reported a failure.
    #[error("database operation failed")]
    Database(#[source] std::sync::Arc<dyn std::error::Error + Send + Sync>),
}

impl DataError {
    /// Wraps a driver error.
    pub fn database(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Database(std::sync::Arc::from(source.into()))
    }

    /// The 500 the host returns, leaking nothing about the cause.
    #[must_use]
    pub fn into_problem(self, trace_id: impl Into<String>) -> ProblemDetails {
        tracing::error!(error = %self, "data access failed");

        ProblemDetails::internal_server_error(trace_id)
    }
}

impl IntoResponse for DataError {
    fn into_response(self) -> Response {
        self.into_problem(current_trace_id()).into_response()
    }
}

/// Result of a repository call.
pub type DataResult<T> = Result<T, DataError>;

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
        let problem = DataError::database("no such table: Album").into_problem("trace-1");

        assert_eq!(problem.status, 500);
        assert_eq!(
            problem.detail, None,
            "the driver's message must not reach the client"
        );
    }

    #[test]
    fn the_driver_error_is_still_available_for_logging() {
        let error = DataError::database("no such table: Album");

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
        let problem = ApiError::from(DataError::database("boom")).into_problem("trace-1");

        assert_eq!(problem.status, 500);
        assert_eq!(problem.detail, None);
    }
}
