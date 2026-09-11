//! RFC 7807 error responses, ported from the host's global exception handler.
//!
//! The C# handler distinguishes four cases, and this module reproduces each
//! one's exact `type`, `title`, and `detail`, because all three are wire-visible:
//!
//! | Trigger | Status | Title |
//! |---|---|---|
//! | `ValidationException` | 400 | `Request validation failed.` |
//! | `BadHttpRequestException` | its own | `Malformed request.` |
//! | `JsonException` | 400 | `Malformed request.` |
//! | anything else | 500 | `An unexpected error occurred.` |
//!
//! Every response carries a `traceId` extension.

use std::collections::BTreeMap;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The `type` the original uses for client errors (RFC 9110 §15.5.1).
pub const CLIENT_ERROR_TYPE: &str = "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1";

/// The `type` the original uses for server errors (RFC 9110 §15.6.1).
pub const SERVER_ERROR_TYPE: &str = "https://www.rfc-editor.org/rfc/rfc9110#section-15.6.1";

/// Title for a failed validation.
pub const VALIDATION_TITLE: &str = "Request validation failed.";

/// Detail for a failed validation.
pub const VALIDATION_DETAIL: &str = "One or more validation errors occurred.";

/// Title for a request the host could not parse.
pub const MALFORMED_REQUEST_TITLE: &str = "Malformed request.";

/// Title for an unhandled failure.
pub const UNEXPECTED_ERROR_TITLE: &str = "An unexpected error occurred.";

/// The media type ProblemDetails responses are served as.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// An RFC 7807 problem document.
///
/// Field names are the ones ASP.NET Core emits. JSON object member order is not
/// significant, and the parity harness compares parsed documents rather than
/// raw text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProblemDetails {
    /// The problem type URI.
    #[serde(rename = "type")]
    pub type_uri: String,

    /// A short, human-readable summary.
    pub title: String,

    /// The HTTP status code.
    pub status: u16,

    /// A human-readable explanation specific to this occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,

    /// The URI reference that identifies the specific occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,

    /// Per-field validation messages, present only on validation failures.
    ///
    /// A `BTreeMap` rather than a hash map so the document is byte-stable
    /// across runs, which keeps the parity diffs readable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<BTreeMap<String, Vec<String>>>,

    /// The request's trace identifier, carried as an RFC 7807 extension.
    #[serde(rename = "traceId")]
    pub trace_id: String,
}

impl ProblemDetails {
    /// A 400 carrying per-field validation messages.
    ///
    /// The C# handler groups `FluentValidation` failures by property name and,
    /// when an exception carries no per-property failures at all, files its
    /// message under the empty key. [`validation_errors_from`] reproduces that
    /// grouping.
    #[must_use]
    pub fn validation(errors: BTreeMap<String, Vec<String>>, trace_id: impl Into<String>) -> Self {
        Self {
            type_uri: CLIENT_ERROR_TYPE.to_owned(),
            title: VALIDATION_TITLE.to_owned(),
            status: StatusCode::BAD_REQUEST.as_u16(),
            detail: Some(VALIDATION_DETAIL.to_owned()),
            instance: None,
            errors: Some(errors),
            trace_id: trace_id.into(),
        }
    }

    /// A request the host could not parse — malformed JSON, or a body that
    /// failed model binding.
    #[must_use]
    pub fn malformed_request(
        status: StatusCode,
        detail: impl Into<String>,
        trace_id: impl Into<String>,
    ) -> Self {
        Self {
            type_uri: CLIENT_ERROR_TYPE.to_owned(),
            title: MALFORMED_REQUEST_TITLE.to_owned(),
            status: status.as_u16(),
            detail: Some(detail.into()),
            instance: None,
            errors: None,
            trace_id: trace_id.into(),
        }
    }

    /// A 500 that deliberately leaks nothing about the underlying failure.
    #[must_use]
    pub fn internal_server_error(trace_id: impl Into<String>) -> Self {
        Self {
            type_uri: SERVER_ERROR_TYPE.to_owned(),
            title: UNEXPECTED_ERROR_TITLE.to_owned(),
            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            detail: None,
            instance: None,
            errors: None,
            trace_id: trace_id.into(),
        }
    }

    /// The status this document reports, falling back to 500 if it somehow
    /// carries a code outside the valid range.
    #[must_use]
    pub fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let mut response = (status, Json(self)).into_response();

        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(PROBLEM_JSON),
        );

        response
    }
}

/// Groups `(field, message)` pairs the way the C# handler groups
/// `ValidationException.Errors`: by property name, preserving order within a
/// field but discarding duplicate messages.
///
/// A failure with a blank property name is filed under the empty key, matching
/// the original's `string.IsNullOrWhiteSpace(error.PropertyName)` branch.
#[must_use]
pub fn validation_errors_from<I, F, M>(failures: I) -> BTreeMap<String, Vec<String>>
where
    I: IntoIterator<Item = (F, M)>,
    F: AsRef<str>,
    M: Into<String>,
{
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (field, message) in failures {
        let key = if field.as_ref().trim().is_empty() {
            String::new()
        } else {
            field.as_ref().to_owned()
        };

        let messages = grouped.entry(key).or_default();
        let message = message.into();

        if !messages.contains(&message) {
            messages.push(message);
        }
    }

    grouped
}

/// Port of the body `UseStatusCodePages()` writes.
///
/// The host installs the status-code-pages middleware, so a response that
/// carries a status in the 400–599 range and *no* body — a bare 404 from
/// `Results.NotFound()`, or a 401 from the authentication challenge — comes
/// back as `text/plain` reading `Status Code: 404; Not Found`. Clients see it,
/// so the port reproduces it rather than returning a genuinely empty body.
#[must_use]
pub fn status_code_page_body(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("Status Code: {}; {reason}", status.as_u16()),
        None => format!("Status Code: {}", status.as_u16()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn to_json(problem: &ProblemDetails) -> serde_json::Value {
        serde_json::to_value(problem).expect("problem details should serialize")
    }

    #[test]
    fn validation_problem_matches_the_original_document() {
        let errors = validation_errors_from([("Name", "'Name' must not be empty.")]);
        let problem = ProblemDetails::validation(errors, "trace-1");

        assert_eq!(
            to_json(&problem),
            serde_json::json!({
                "type": "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1",
                "title": "Request validation failed.",
                "status": 400,
                "detail": "One or more validation errors occurred.",
                "errors": { "Name": ["'Name' must not be empty."] },
                "traceId": "trace-1"
            })
        );
    }

    #[test]
    fn server_error_leaks_no_detail() {
        let problem = ProblemDetails::internal_server_error("trace-2");
        let document = to_json(&problem);

        assert_eq!(document["status"], 500);
        assert_eq!(document["title"], "An unexpected error occurred.");
        assert!(
            document.get("detail").is_none(),
            "the original sends no detail on a 500, to avoid leaking internals"
        );
    }

    #[test]
    fn absent_fields_are_omitted_rather_than_null() {
        // ASP.NET Core omits unset ProblemDetails members; emitting explicit
        // nulls would be a visible difference.
        let document = to_json(&ProblemDetails::malformed_request(
            StatusCode::BAD_REQUEST,
            "Unexpected end of JSON input.",
            "trace-3",
        ));

        assert!(document.get("instance").is_none());
        assert!(document.get("errors").is_none());
        assert_eq!(document["detail"], "Unexpected end of JSON input.");
    }

    #[test]
    fn failures_group_by_field_and_drop_duplicates() {
        let errors = validation_errors_from([
            ("Name", "must not be empty"),
            ("Name", "must not be empty"),
            ("Name", "too long"),
            ("Title", "required"),
        ]);

        assert_eq!(errors["Name"], vec!["must not be empty", "too long"]);
        assert_eq!(errors["Title"], vec!["required"]);
    }

    #[test]
    fn blank_field_names_collapse_to_the_empty_key() {
        let errors = validation_errors_from([("   ", "something went wrong")]);

        assert_eq!(errors[""], vec!["something went wrong"]);
    }

    #[test]
    fn status_code_pages_body_matches_the_middleware() {
        assert_eq!(
            status_code_page_body(StatusCode::NOT_FOUND),
            "Status Code: 404; Not Found"
        );
        assert_eq!(
            status_code_page_body(StatusCode::UNAUTHORIZED),
            "Status Code: 401; Unauthorized"
        );
    }
}
