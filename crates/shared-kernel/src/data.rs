//! Data-access failures and the responses they produce.
//!
//! [`RepositoryError`] lives here rather than beside the repository traits so
//! that this crate can turn one into a response without depending on the crate
//! that owns the traits. `shared-persistence` re-exports it, so callers there
//! see it where they expect to.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::ProblemDetails;
use crate::errors::new_trace_id;

/// Something went wrong talking to the database.
///
/// The driver's own error type is boxed because neither this crate nor the
/// contracts crate may depend on a driver. The C# equivalent is an unhandled
/// exception, which the host turns into a 500.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    /// The underlying database reported a failure.
    #[error("database operation failed")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl RepositoryError {
    /// Wraps a driver error.
    pub fn database(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Database(source.into())
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
        self.into_problem(new_trace_id()).into_response()
    }
}

/// Result of a repository call.
pub type RepositoryResult<T> = Result<T, RepositoryError>;

/// Renders a by-id lookup.
///
/// Port of `album is not null ? TypedResults.Ok(album) : Results.NotFound()`.
/// The 404 carries no body of its own — the status-code-pages layer fills in
/// the problem document, exactly as it does in the original.
pub fn item_response<T: Serialize>(result: RepositoryResult<Option<T>>) -> Response {
    match result {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => error.into_response(),
    }
}

/// Renders a collection lookup.
///
/// Always 200, even when empty. The original's endpoints declare a 404 here,
/// but `Results.Json(...)` cannot produce one — an empty result is an empty
/// array. That declaration is documentation-only, and this reproduces the
/// behavior rather than the declaration.
pub fn collection_response<T: Serialize>(result: RepositoryResult<Vec<T>>) -> Response {
    match result {
        Ok(values) => Json(values).into_response(),
        Err(error) => error.into_response(),
    }
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
    fn a_found_item_is_serialized_and_a_missing_one_is_a_bodiless_404() {
        let found = item_response(Ok(Some(serde_json::json!({ "Id": 1 }))));
        assert_eq!(found.status(), StatusCode::OK);

        let missing = item_response::<serde_json::Value>(Ok(None));
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(
            !missing
                .headers()
                .contains_key(axum::http::header::CONTENT_TYPE),
            "the status-code-pages layer supplies the body, not the handler"
        );
    }

    #[test]
    fn an_empty_collection_is_still_a_200() {
        // The endpoints declare a 404 for collections, but the original cannot
        // produce one: an empty result is an empty array.
        let response = collection_response::<serde_json::Value>(Ok(Vec::new()));

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn a_failure_becomes_a_500_on_both_paths() {
        assert_eq!(
            item_response::<serde_json::Value>(Err(RepositoryError::database("boom"))).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            collection_response::<serde_json::Value>(Err(RepositoryError::database("boom")))
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
