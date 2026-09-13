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

use crate::data::RepositoryError;
use crate::json::MalformedRequest;

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
    ///
    /// Optional because ASP.NET's defaults table has no entry for some statuses
    /// — 429 among them — and omits the member entirely rather than sending a
    /// placeholder.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_uri: Option<String>,

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
            type_uri: Some(CLIENT_ERROR_TYPE.to_owned()),
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
            type_uri: Some(CLIENT_ERROR_TYPE.to_owned()),
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
            type_uri: Some(SERVER_ERROR_TYPE.to_owned()),
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

/// Port of what `UseStatusCodePages()` writes for a bodiless 4xx or 5xx.
///
/// The middleware's own default is plain text — `Status Code: 404; Not Found` —
/// but the host also calls `AddProblemDetails()`, which replaces that with a
/// problem document. Verified against the running service: a bare
/// `Results.NotFound()`, an auth challenge, and a rate-limit rejection all come
/// back as `application/problem+json`.
///
/// Note the `type` vocabulary differs from the custom exception handler's. This
/// table uses `tools.ietf.org`; [`CLIENT_ERROR_TYPE`] and [`SERVER_ERROR_TYPE`]
/// use `www.rfc-editor.org`. Both appear in the same service.
#[must_use]
pub fn status_code_page(status: StatusCode, trace_id: impl Into<String>) -> ProblemDetails {
    let (type_uri, title) = problem_defaults(status);

    ProblemDetails {
        type_uri: type_uri.map(ToOwned::to_owned),
        title: title.to_owned(),
        status: status.as_u16(),
        detail: None,
        instance: None,
        errors: None,
        trace_id: trace_id.into(),
    }
}

/// The `type` ASP.NET Core's defaults table assigns to a status, if any.
///
/// `Results.Problem(...)` called without an explicit `type` fills this in, so
/// a hand-titled problem still carries the framework's URI — which is the
/// `tools.ietf.org` vocabulary, not the `www.rfc-editor.org` one the custom
/// exception handler passes explicitly.
#[must_use]
pub fn default_problem_type(status: StatusCode) -> Option<&'static str> {
    problem_defaults(status).0
}

/// ASP.NET Core's `ProblemDetailsDefaults` table.
///
/// A status in the table gets its `type` and a fixed `title`. Anything else —
/// 429 among them — gets **no `type` at all** and falls back to the reason
/// phrase for its title.
fn problem_defaults(status: StatusCode) -> (Option<&'static str>, &'static str) {
    let entry = match status.as_u16() {
        400 => Some(("#section-15.5.1", "Bad Request")),
        401 => Some(("#section-15.5.2", "Unauthorized")),
        403 => Some(("#section-15.5.4", "Forbidden")),
        404 => Some(("#section-15.5.5", "Not Found")),
        405 => Some(("#section-15.5.6", "Method Not Allowed")),
        406 => Some(("#section-15.5.7", "Not Acceptable")),
        409 => Some(("#section-15.5.10", "Conflict")),
        415 => Some(("#section-15.5.16", "Unsupported Media Type")),
        422 => Some(("#section-15.5.21", "Unprocessable Entity")),
        426 => Some(("#section-15.5.22", "Upgrade Required")),
        500 => Some((
            "#section-15.6.1",
            "An error occurred while processing your request.",
        )),
        _ => None,
    };

    match entry {
        Some((fragment, title)) => (Some(default_type_for(fragment)), title),
        None => (None, status.canonical_reason().unwrap_or("Error")),
    }
}

/// Resolves a fragment against the default table's base URI.
fn default_type_for(fragment: &str) -> &'static str {
    // `const` rather than `format!` so the returned reference is 'static; the
    // set is closed and small.
    match fragment {
        "#section-15.5.1" => "https://tools.ietf.org/html/rfc9110#section-15.5.1",
        "#section-15.5.2" => "https://tools.ietf.org/html/rfc9110#section-15.5.2",
        "#section-15.5.4" => "https://tools.ietf.org/html/rfc9110#section-15.5.4",
        "#section-15.5.5" => "https://tools.ietf.org/html/rfc9110#section-15.5.5",
        "#section-15.5.6" => "https://tools.ietf.org/html/rfc9110#section-15.5.6",
        "#section-15.5.7" => "https://tools.ietf.org/html/rfc9110#section-15.5.7",
        "#section-15.5.10" => "https://tools.ietf.org/html/rfc9110#section-15.5.10",
        "#section-15.5.16" => "https://tools.ietf.org/html/rfc9110#section-15.5.16",
        "#section-15.5.21" => "https://tools.ietf.org/html/rfc9110#section-15.5.21",
        "#section-15.5.22" => "https://tools.ietf.org/html/rfc9110#section-15.5.22",
        _ => "https://tools.ietf.org/html/rfc9110#section-15.6.1",
    }
}

tokio::task_local! {
    /// The identifier every problem document raised while serving one request
    /// reports as its `traceId`.
    static REQUEST_TRACE_ID: String;
}

/// Runs `work` with `trace_id` as the ambient request identifier.
///
/// The host's request-id layer wraps every request in one of these, so any
/// problem document raised anywhere inside — a handler's 404, a rejected
/// extractor, the status-code-pages layer, the panic handler — reports the
/// same `traceId` the response header and the log records carry. Without it
/// each of those minted its own identifier and none of them agreed.
pub async fn with_trace_id<F>(trace_id: String, work: F) -> F::Output
where
    F: std::future::Future,
{
    REQUEST_TRACE_ID.scope(trace_id, work).await
}

/// The current request's identifier, or a fresh one outside a request.
///
/// Every response-building path calls this rather than [`new_trace_id`], so
/// the only documents carrying an unrelated identifier are the ones built
/// outside a request — in tests, mostly.
#[must_use]
pub fn current_trace_id() -> String {
    REQUEST_TRACE_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| new_trace_id())
}

/// Whether a header value is a usable W3C `traceparent`.
///
/// An inbound identifier is honored so a `traceId` in a response can be traced
/// back through whatever proxied the call, but only when it is really one of
/// these — an arbitrary client-supplied string would otherwise end up in the
/// logs as though the service had minted it.
#[must_use]
pub fn is_traceparent(value: &str) -> bool {
    let segments: Vec<&str> = value.split('-').collect();

    segments.len() == 4
        && segments[0].len() == 2
        && segments[1].len() == 32
        && segments[2].len() == 16
        && segments[3].len() == 2
        && segments
            .iter()
            .all(|segment| segment.chars().all(|c| c.is_ascii_hexdigit()))
        && segments[1].chars().any(|c| c != '0')
        && segments[2].chars().any(|c| c != '0')
}

/// Why a request could not be answered.
///
/// This is the error half of every handler's `Result`. The C# original has no
/// equivalent: there, a handler either returns an `IResult` it built itself or
/// throws, and the host's exception handler sorts out what that becomes. The
/// shapes below are exactly the ones that handler produces, so the wire output
/// is unchanged — what changes is that a handler now *returns* its refusal
/// instead of deciding, statement by statement, to send one.
#[derive(Debug)]
pub enum ApiError {
    /// No usable token: 401 with a `WWW-Authenticate` challenge.
    Unauthenticated,

    /// Authenticated, but a requirement failed: 403.
    Forbidden,

    /// Nothing with that key: 404, from the defaults table.
    NotFound,

    /// A model broke a validation rule: 400 carrying the per-field messages.
    Validation(BTreeMap<String, Vec<String>>),

    /// The host could not read the request body.
    Malformed(MalformedRequest),

    /// The database refused: 500, leaking nothing about the cause.
    Repository(RepositoryError),
}

impl ApiError {
    /// The problem document this becomes, under an explicit trace identifier.
    #[must_use]
    pub fn into_problem(self, trace_id: impl Into<String>) -> ProblemDetails {
        let trace_id = trace_id.into();

        match self {
            Self::Unauthenticated => status_code_page(StatusCode::UNAUTHORIZED, trace_id),
            Self::Forbidden => status_code_page(StatusCode::FORBIDDEN, trace_id),
            Self::NotFound => status_code_page(StatusCode::NOT_FOUND, trace_id),
            Self::Validation(errors) => ProblemDetails::validation(errors, trace_id),
            Self::Malformed(malformed) => malformed.into_problem_with(trace_id),
            Self::Repository(error) => error.into_problem(trace_id),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthenticated => formatter.write_str("unauthenticated"),
            Self::Forbidden => formatter.write_str("forbidden"),
            Self::NotFound => formatter.write_str("not found"),
            Self::Validation(_) => formatter.write_str("request validation failed"),
            Self::Malformed(_) => formatter.write_str("malformed request"),
            Self::Repository(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RepositoryError> for ApiError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<MalformedRequest> for ApiError {
    fn from(malformed: MalformedRequest) -> Self {
        Self::Malformed(malformed)
    }
}

impl From<std::convert::Infallible> for ApiError {
    fn from(infallible: std::convert::Infallible) -> Self {
        match infallible {}
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let challenge = matches!(self, Self::Unauthenticated);
        let mut response = self.into_problem(current_trace_id()).into_response();

        if challenge {
            // The JWT challenge writes this; verified on the running service.
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }

        response
    }
}

/// Builds a W3C `traceparent`-shaped identifier.
///
/// ASP.NET's `HttpContext.TraceIdentifier` reports the ambient activity, so
/// every `traceId` on the wire reads `00-{32 hex}-{16 hex}-00`. Nothing depends
/// on the value; matching the shape keeps responses recognizable.
#[must_use]
pub fn new_trace_id() -> String {
    let trace: u128 = rand::random();
    let span: u64 = rand::random();

    format!("00-{trace:032x}-{span:016x}-00")
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

    /// Captured from the running C# service, which is the only reason these
    /// are known: the middleware's documented default is plain text, and
    /// `AddProblemDetails()` silently replaces it.
    #[test]
    fn status_code_pages_match_the_documents_the_service_returns() {
        assert_eq!(
            to_json(&status_code_page(StatusCode::NOT_FOUND, "trace-1")),
            serde_json::json!({
                "type": "https://tools.ietf.org/html/rfc9110#section-15.5.5",
                "title": "Not Found",
                "status": 404,
                "traceId": "trace-1"
            })
        );

        assert_eq!(
            to_json(&status_code_page(StatusCode::UNAUTHORIZED, "trace-2")),
            serde_json::json!({
                "type": "https://tools.ietf.org/html/rfc9110#section-15.5.2",
                "title": "Unauthorized",
                "status": 401,
                "traceId": "trace-2"
            })
        );

        assert_eq!(
            to_json(&status_code_page(StatusCode::FORBIDDEN, "trace-3"))["type"],
            "https://tools.ietf.org/html/rfc9110#section-15.5.4"
        );
    }

    #[test]
    fn a_status_outside_the_defaults_table_carries_no_type() {
        // 429 has no entry, so the document omits `type` entirely and takes its
        // title from the reason phrase.
        let document = to_json(&status_code_page(StatusCode::TOO_MANY_REQUESTS, "trace-4"));

        assert_eq!(
            document,
            serde_json::json!({
                "title": "Too Many Requests",
                "status": 429,
                "traceId": "trace-4"
            })
        );
    }

    #[test]
    fn the_two_type_vocabularies_stay_distinct() {
        // The defaults table uses tools.ietf.org; the custom exception handler
        // uses www.rfc-editor.org. Both appear in the same service.
        let from_table = to_json(&status_code_page(StatusCode::BAD_REQUEST, "trace-5"));
        let from_handler = to_json(&ProblemDetails::validation(BTreeMap::new(), "trace-6"));

        assert_eq!(
            from_table["type"],
            "https://tools.ietf.org/html/rfc9110#section-15.5.1"
        );
        assert_eq!(
            from_handler["type"],
            "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1"
        );
    }

    #[test]
    fn trace_identifiers_look_like_w3c_traceparents() {
        let trace = new_trace_id();
        let segments: Vec<&str> = trace.split('-').collect();

        assert_eq!(segments.len(), 4, "{trace}");
        assert_eq!(segments[0], "00");
        assert_eq!(segments[1].len(), 32, "{trace}");
        assert_eq!(segments[2].len(), 16, "{trace}");
        assert_eq!(segments[3], "00");
        assert!(
            segments[1].chars().all(|c| c.is_ascii_hexdigit()),
            "{trace}"
        );
    }

    #[test]
    fn trace_identifiers_actually_vary_across_their_whole_width() {
        // An earlier version derived both halves from a counter, which left
        // the span identical on every response and the trace mostly zeroes.
        // Length checks alone did not catch it.
        let samples: Vec<String> = (0..16).map(|_| new_trace_id()).collect();

        let traces: BTreeMap<&str, ()> = samples
            .iter()
            .map(|id| (id.split('-').nth(1).unwrap_or_default(), ()))
            .collect();
        let spans: BTreeMap<&str, ()> = samples
            .iter()
            .map(|id| (id.split('-').nth(2).unwrap_or_default(), ()))
            .collect();

        assert_eq!(traces.len(), samples.len(), "every trace should differ");
        assert_eq!(spans.len(), samples.len(), "every span should differ");
        assert!(
            !samples
                .iter()
                .any(|id| id.contains("-0000000000000000") || id.contains("00000000000000000000")),
            "a run of zeroes suggests the identifier is not really random: {samples:?}"
        );
    }
}
