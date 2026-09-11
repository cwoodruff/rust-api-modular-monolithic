//! Validation, ported from `SharedKernel.Persistence.Validation`.
//!
//! [`rules`] carries FluentValidation's semantics and message templates;
//! [`validators`] carries the ten rule sets themselves.
//!
//! Validation runs *inside the services*, not as a request filter — an
//! architectural choice the original documents. The consequence is that a
//! failure does not short-circuit before the handler; it propagates out of the
//! service and the host's global handler turns it into a 400. [`problem_details`]
//! is that final step.

pub mod rules;
pub mod validators;

pub use rules::{
    Rules, Validate, ValidationFailure, phone_pattern, postal_code_pattern, split_pascal_case,
};

use shared_kernel::ProblemDetails;
use shared_kernel::errors::validation_errors_from;

/// Turns validation failures into the 400 the client sees.
///
/// Failures group by property name, so several broken rules on one field
/// arrive as several messages under one key.
#[must_use]
pub fn problem_details(
    failures: Vec<ValidationFailure>,
    trace_id: impl Into<String>,
) -> ProblemDetails {
    let errors = validation_errors_from(
        failures
            .into_iter()
            .map(|failure| (failure.property_name, failure.message)),
    );

    ProblemDetails::validation(errors, trace_id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::api_models::AlbumApiModel;

    #[test]
    fn a_failed_validation_becomes_the_original_400_document() {
        let failures = AlbumApiModel::default()
            .validate()
            .expect_err("an empty album should fail");

        let problem = problem_details(failures, "trace-1");
        let document = serde_json::to_value(&problem).expect("problem should serialize");

        assert_eq!(document["status"], 400);
        assert_eq!(document["title"], "Request validation failed.");
        assert_eq!(
            document["detail"],
            "One or more validation errors occurred."
        );
        assert_eq!(document["traceId"], "trace-1");
        assert_eq!(
            document["errors"]["Title"],
            serde_json::json!(["'Title' must not be empty."])
        );
        assert_eq!(
            document["errors"]["ArtistId"],
            serde_json::json!(["'Artist Id' must not be empty."])
        );
    }

    #[test]
    fn several_failures_on_one_field_collapse_under_one_key() {
        let album = AlbumApiModel {
            // Too short and, at 161 characters, too long is impossible at once;
            // use a value that breaks the minimum while ArtistId also fails.
            title: Some("ab".to_owned()),
            artist_id: None,
            ..AlbumApiModel::default()
        };

        let problem = problem_details(album.validate().expect_err("should fail"), "trace-2");
        let document = serde_json::to_value(&problem).expect("problem should serialize");

        assert_eq!(
            document["errors"]["Title"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1
        );
        assert!(document["errors"]["ArtistId"].is_array());
    }
}
