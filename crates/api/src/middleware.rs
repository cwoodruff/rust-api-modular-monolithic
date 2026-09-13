//! The middleware pipeline, ported from `Program.cs`.
//!
//! Order matters and is preserved. In the original, reading downward:
//!
//! 1. HSTS and HTTPS redirection — non-Development only
//! 2. the exception handler, producing ProblemDetails
//! 3. `UseStatusCodePages()`
//! 4. the security-headers middleware
//! 5. CORS
//! 6. the rate limiter
//! 7. authentication and authorization
//! 8. Swagger — Development and Demo only
//!
//! In axum the outermost layer runs first, so the stack is applied in reverse
//! of this list.
//!
//! One consequence worth naming: the rate limiter sits *before* authentication,
//! so no request has claims by the time its partition key is computed and every
//! caller is bucketed by address. That is the original's behavior, kept.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use shared_kernel::errors::{
    ProblemDetails, current_trace_id, is_traceparent, new_trace_id, status_code_page, with_trace_id,
};
use shared_kernel::traffic_control::{self, PUBLIC_ANON_PERMIT_LIMIT, PUBLIC_ANON_WINDOW};
use tracing::Instrument;

/// The six headers the original sets on every response, labelled "OWASP A05".
///
/// Hard-coded there and hard-coded here: the `SecureHeaders:*` configuration
/// its documentation proposes was never built.
const SECURITY_HEADERS: [(HeaderName, &str); 6] = [
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::X_FRAME_OPTIONS, "DENY"),
    // Deliberately "0": modern browsers treat any other value as harmful, and
    // the CSP below is what actually does this job.
    (header::X_XSS_PROTECTION, "0"),
    (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
    (
        header::CONTENT_SECURITY_POLICY,
        "default-src 'self'; frame-ancestors 'none'",
    ),
    (
        HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()",
    ),
];

/// The origins the `Default` CORS policy allows.
///
/// A hard-coded list of local development front ends, exactly as written.
pub const ALLOWED_ORIGINS: [&str; 6] = [
    "http://localhost:3000",
    "http://localhost:4200",
    "http://localhost:5173",
    "https://localhost:3000",
    "https://localhost:4200",
    "https://localhost:5173",
];

/// The header the request's trace identifier is echoed on.
///
/// The original sets no such header: its `traceId` appears only in the body of
/// a problem document, so a 200 carries no identifier at all and a client
/// reporting a slow or wrong response has nothing to quote. This adds one
/// without changing any body.
pub const TRACE_ID_HEADER: HeaderName = HeaderName::from_static("x-trace-id");

/// The inbound header a caller can use to continue an existing trace.
pub const TRACEPARENT_HEADER: HeaderName = HeaderName::from_static("traceparent");

/// Establishes the request's trace identifier.
///
/// This is what makes the `traceId` in a problem document worth anything.
/// Before it, every response-building path minted its own identifier: the body
/// of a 500 named one trace, the log record written while handling it named
/// none, and nothing tied the two together — so the one field whose entire
/// purpose is correlation correlated with nothing.
///
/// One identifier is now established per request and used three ways: it is
/// the ambient value [`current_trace_id`] returns, so every problem document
/// raised anywhere inside reports it; it is a field on the `tracing` span that
/// wraps the request, so every log record emitted while handling it carries it;
/// and it goes out on [`TRACE_ID_HEADER`], so a caller can quote it.
///
/// A caller's own `traceparent` is honored when it really is one, so a trace
/// that started upstream continues rather than restarting here.
pub async fn request_id(request: Request, next: Next) -> Response {
    let inbound = request
        .headers()
        .get(&TRACEPARENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| is_traceparent(value))
        .map(ToOwned::to_owned);

    let trace_id = inbound.unwrap_or_else(new_trace_id);

    let span = tracing::info_span!(
        "request",
        trace_id = %trace_id,
        method = %request.method(),
        path = %request.uri().path(),
    );

    let mut response = with_trace_id(trace_id.clone(), next.run(request).instrument(span)).await;

    if let Ok(value) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert(TRACE_ID_HEADER, value);
    }

    response
}

/// Sets the security headers on every response.
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    for (name, value) in SECURITY_HEADERS {
        headers.insert(name, HeaderValue::from_static(value));
    }

    response
}

/// Port of `UseStatusCodePages()` as the host actually configures it.
///
/// The middleware's plain-text default is replaced by `AddProblemDetails()`, so
/// a bodiless 4xx or 5xx comes back as `application/problem+json`. Verified
/// against the running service — a bare 404, an auth challenge, and a
/// rate-limit rejection all answer with a problem document.
pub async fn status_code_pages(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();

    let eligible = status.is_client_error() || status.is_server_error();
    let has_content_type = response.headers().contains_key(header::CONTENT_TYPE);

    if !eligible || has_content_type {
        return response;
    }

    let (parts, body) = response.into_parts();

    // Only an empty body qualifies; anything already written is left alone.
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return (parts, Body::empty()).into_response(),
    };

    if !bytes.is_empty() {
        return (parts, Body::from(bytes)).into_response();
    }

    let problem = status_code_page(status, trace_id()).into_response();
    let (problem_parts, problem_body) = problem.into_parts();

    // Keep the headers the inner layers set — `WWW-Authenticate` on a
    // challenge, most importantly — and take the problem document's
    // content type and body.
    let mut merged = parts;
    merged.headers.extend(problem_parts.headers);

    (merged, problem_body).into_response()
}

/// The content type ASP.NET Core writes for a JSON body.
pub const JSON_WITH_CHARSET: &str = "application/json; charset=utf-8";

/// Appends `charset=utf-8` to JSON responses.
///
/// ASP.NET Core always writes `application/json; charset=utf-8`, while axum
/// writes a bare `application/json`. The difference is wire-visible, and a
/// client that compares the header exactly would see it.
///
/// Only plain JSON is rewritten. Problem documents keep their bare
/// `application/problem+json`, which is what the original sends for those —
/// verified against the running service, where a 404 and a validation failure
/// both carry no charset.
pub async fn json_charset(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let is_plain_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "application/json");

    if is_plain_json {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_WITH_CHARSET),
        );
    }

    response
}

/// Adds `Strict-Transport-Security`, as `UseHsts()` does outside Development.
///
/// The original pairs this with `UseHttpsRedirection()`, which 307s plain HTTP
/// to HTTPS. That redirect is left to the edge here — the original's own
/// deployment guide recommends terminating TLS upstream, and reproducing the
/// in-process redirect is what makes its Docker image unable to serve traffic.
pub async fn hsts(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    response.headers_mut().insert(
        HeaderName::from_static("strict-transport-security"),
        // ASP.NET Core's defaults: 30 days, no subdomains, no preload.
        HeaderValue::from_static("max-age=2592000"),
    );

    response
}

/// One partition's allowance within the current window.
#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    used: u32,
}

/// The fixed-window limiter behind `global:public-anon`.
///
/// 60 requests per 60 seconds per partition, queue depth zero so rejection is
/// immediate, answering 429. The original writes no `Retry-After`, and neither
/// does this.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    windows: Arc<DashMap<String, Window>>,
    permit_limit: u32,
    window: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(PUBLIC_ANON_PERMIT_LIMIT, PUBLIC_ANON_WINDOW)
    }
}

impl RateLimiter {
    /// Builds a limiter with explicit limits, for tests.
    #[must_use]
    pub fn new(permit_limit: u32, window: Duration) -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
            permit_limit,
            window,
        }
    }

    /// Records a request against `partition`, reporting whether it is allowed.
    fn try_acquire(&self, partition: &str) -> bool {
        let now = Instant::now();

        let mut entry = self.windows.entry(partition.to_owned()).or_insert(Window {
            started: now,
            used: 0,
        });

        if now.duration_since(entry.started) >= self.window {
            entry.started = now;
            entry.used = 0;
        }

        if entry.used >= self.permit_limit {
            return false;
        }

        entry.used += 1;
        true
    }
}

/// Applies the rate limiter, rejecting with 429.
pub async fn rate_limit(
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    request: Request,
    next: Next,
) -> Response {
    let address = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(socket)| socket.ip());

    // Claims are always absent here: the limiter runs before authentication,
    // as it does in the original.
    let partition = traffic_control::partition_key(None, address);

    if limiter.try_acquire(&partition) {
        next.run(request).await
    } else {
        StatusCode::TOO_MANY_REQUESTS.into_response()
    }
}

/// Turns a panic into the same 500 the original's handler produces for an
/// unhandled exception.
///
/// The message is discarded rather than returned: the C# handler passes no
/// `detail` on a 500 precisely so internals do not reach the client.
#[must_use]
pub fn panic_to_problem(panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());

    tracing::error!(%detail, "unhandled panic");

    ProblemDetails::internal_server_error(trace_id()).into_response()
}

/// The identifier this response should report.
///
/// Inside a request this is the one [`request_id`] established, so the document
/// a layer writes names the same trace the log records do. Outside one — in a
/// test that calls a layer directly — it falls back to a fresh identifier.
///
/// Verified against the running service: every `traceId` on the wire is a W3C
/// `traceparent`, because `HttpContext.TraceIdentifier` reports the ambient
/// activity rather than Kestrel's connection counter.
#[must_use]
pub fn trace_id() -> String {
    current_trace_id()
}

/// Whether a request arrived over a secure transport.
///
/// Used only to decide whether HSTS applies. `X-Forwarded-Proto` is honored,
/// which the original does not do — it never configures forwarded headers, so
/// behind a TLS-terminating proxy its redirect loops.
#[must_use]
pub fn is_secure(uri: &Uri, forwarded_proto: Option<&str>) -> bool {
    if let Some(proto) = forwarded_proto {
        return proto.eq_ignore_ascii_case("https");
    }

    uri.scheme_str() == Some("https")
}

/// The address a partition key falls back to when there is no connection info.
#[must_use]
pub fn unknown_address() -> Option<IpAddr> {
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_limiter_allows_exactly_the_permitted_number() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(
            !limiter.try_acquire("ip:198.51.100.1"),
            "the fourth request should be rejected"
        );
    }

    #[test]
    fn partitions_have_independent_allowances() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(!limiter.try_acquire("ip:198.51.100.1"));
        assert!(
            limiter.try_acquire("ip:198.51.100.2"),
            "a different partition has its own budget"
        );
    }

    #[test]
    fn the_window_resets_once_it_has_elapsed() {
        let limiter = RateLimiter::new(1, Duration::from_millis(1));

        assert!(limiter.try_acquire("ip:198.51.100.1"));
        assert!(!limiter.try_acquire("ip:198.51.100.1"));

        std::thread::sleep(Duration::from_millis(5));

        assert!(
            limiter.try_acquire("ip:198.51.100.1"),
            "a fixed window starts over rather than sliding"
        );
    }

    #[test]
    fn the_default_limiter_uses_the_original_numbers() {
        let limiter = RateLimiter::default();

        assert_eq!(limiter.permit_limit, 60);
        assert_eq!(limiter.window, Duration::from_secs(60));
    }

    #[test]
    fn the_json_content_type_matches_what_asp_net_writes() {
        assert_eq!(JSON_WITH_CHARSET, "application/json; charset=utf-8");
    }

    #[test]
    fn forwarded_proto_decides_whether_a_request_is_secure() {
        let uri: Uri = "http://localhost:5043/".parse().unwrap();

        assert!(!is_secure(&uri, None));
        assert!(is_secure(&uri, Some("https")));
        assert!(is_secure(&uri, Some("HTTPS")));
        assert!(!is_secure(&uri, Some("http")));
    }

    #[test]
    fn trace_identifiers_are_distinct() {
        assert_ne!(trace_id(), trace_id());
    }
}
