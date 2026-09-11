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
use shared_kernel::errors::{ProblemDetails, status_code_page_body};
use shared_kernel::traffic_control::{self, PUBLIC_ANON_PERMIT_LIMIT, PUBLIC_ANON_WINDOW};

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

/// Sets the security headers on every response.
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    for (name, value) in SECURITY_HEADERS {
        headers.insert(name, HeaderValue::from_static(value));
    }

    response
}

/// Port of `UseStatusCodePages()`.
///
/// Gives a bodiless 4xx or 5xx a `text/plain` body reading
/// `Status Code: 404; Not Found`. Without this a bare `Results.NotFound()`
/// would answer with nothing at all, and clients do see the difference.
pub async fn status_code_pages(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();

    let eligible = status.is_client_error() || status.is_server_error();
    let has_content_type = response.headers().contains_key(header::CONTENT_TYPE);

    if !eligible || has_content_type {
        return response;
    }

    let (mut parts, body) = response.into_parts();

    // Only an empty body qualifies; anything already written is left alone.
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return (parts, Body::empty()).into_response(),
    };

    if !bytes.is_empty() {
        return (parts, Body::from(bytes)).into_response();
    }

    parts
        .headers
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));

    (parts, Body::from(status_code_page_body(status))).into_response()
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

/// A correlation identifier for one response.
///
/// The original uses `HttpContext.TraceIdentifier`, which is Kestrel's
/// per-request connection-and-request counter. Nothing depends on its format,
/// only on its presence.
#[must_use]
pub fn trace_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    format!("R{}", COUNTER.fetch_add(1, Ordering::Relaxed))
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
