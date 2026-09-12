//! The five auth endpoints, ported from `AuthEndpoints`.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use shared_kernel::auth::unauthorized;
use shared_kernel::errors::{default_problem_type, new_trace_id};
use shared_kernel::{AuthorizationFailure, Principal, ProblemDetails, Requirement};

use crate::runtime::IdentityRuntime;
use crate::tokens::TokenPair;

/// The login request body. Member names are lowercase, as the record declares.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    /// The login name.
    #[serde(default)]
    pub username: String,
    /// The password.
    #[serde(default)]
    pub password: String,
}

/// The refresh request body.
#[derive(Debug, Clone, Deserialize)]
pub struct RefreshRequest {
    /// The user whose token is being exchanged.
    #[serde(default, rename = "userId")]
    pub user_id: String,
    /// The refresh token.
    #[serde(default, rename = "refreshToken")]
    pub refresh_token: String,
}

/// The logout request body.
#[derive(Debug, Clone, Deserialize)]
pub struct LogoutRequest {
    /// The user whose token is being revoked.
    #[serde(default, rename = "userId")]
    pub user_id: String,
    /// The refresh token to revoke.
    #[serde(default, rename = "refreshToken")]
    pub refresh_token: String,
}

/// The token envelope.
///
/// Snake-case members, unlike the PascalCase API models — these come from a C#
/// anonymous object and the host's naming policy leaves both alone.
#[derive(Debug, Clone, Serialize)]
pub struct TokenResponse {
    /// The signed JWT.
    pub access_token: String,
    /// Always `Bearer`.
    pub token_type: String,
    /// When the access token expires.
    ///
    /// A `DateTimeOffset` in the original, so it renders with a `+00:00` offset
    /// rather than `Z`, and with trailing zeros trimmed from the fraction —
    /// verified against the running service.
    pub expires_at_utc: String,
    /// The opaque refresh token.
    pub refresh_token: String,
}

impl TokenResponse {
    /// Wraps an issued pair.
    #[must_use]
    pub fn from_pair(pair: TokenPair) -> Self {
        Self {
            access_token: pair.access_token,
            token_type: "Bearer".to_owned(),
            expires_at_utc: format_offset(pair.expires_at_utc),
            refresh_token: pair.refresh_token,
        }
    }
}

/// Renders an instant the way `System.Text.Json` renders a `DateTimeOffset`.
///
/// Up to seven fractional digits with trailing zeros removed, then the offset.
/// A whole second drops the fraction entirely.
fn format_offset(instant: chrono::DateTime<chrono::Utc>) -> String {
    let ticks = instant.timestamp_subsec_nanos() / 100;
    let base = instant.format("%Y-%m-%dT%H:%M:%S");

    if ticks == 0 {
        return format!("{base}+00:00");
    }

    let fraction = format!("{ticks:07}");
    let trimmed = fraction.trim_end_matches('0');

    format!("{base}.{trimmed}+00:00")
}

/// The `Invalid request` problem both blank-field checks return.
///
/// `Results.Problem(...)` is called without a `type`, so the framework fills
/// in the one from its defaults table — the `tools.ietf.org` vocabulary, not
/// the `www.rfc-editor.org` one the custom exception handler passes. Verified
/// against the running service.
fn invalid_request(detail: &str) -> ProblemDetails {
    ProblemDetails {
        type_uri: default_problem_type(StatusCode::BAD_REQUEST).map(ToOwned::to_owned),
        title: "Invalid request".to_owned(),
        status: StatusCode::BAD_REQUEST.as_u16(),
        detail: Some(detail.to_owned()),
        instance: None,
        errors: None,
        trace_id: new_trace_id(),
    }
}

/// Mounts the auth endpoints.
///
/// Note where JWKS lands: inside the module group, so its real path is
/// `/api/identity/.well-known/jwks.json` rather than the conventional root
/// one. That is the original's arrangement and it is wire-visible.
pub fn routes<S>(runtime: Arc<IdentityRuntime>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let login = Arc::clone(&runtime);
    let refresh = Arc::clone(&runtime);
    let logout = Arc::clone(&runtime);
    let jwks = Arc::clone(&runtime);

    Router::new()
        .route(
            "/login",
            post(move |Json(request): Json<LoginRequest>| {
                let runtime = Arc::clone(&login);
                async move { handle_login(&runtime, request) }
            }),
        )
        .route(
            "/refresh",
            post(move |Json(request): Json<RefreshRequest>| {
                let runtime = Arc::clone(&refresh);
                async move { handle_refresh(&runtime, request) }
            }),
        )
        .route(
            "/logout",
            post(
                move |principal: Principal, Json(request): Json<LogoutRequest>| {
                    let runtime = Arc::clone(&logout);
                    async move { handle_logout(&runtime, &principal, request) }
                },
            ),
        )
        .route("/userinfo", get(handle_userinfo))
        .route(
            "/.well-known/jwks.json",
            get(move || {
                let runtime = Arc::clone(&jwks);
                async move { Json(runtime.tokens().jwks()) }
            }),
        )
}

fn handle_login(runtime: &IdentityRuntime, request: LoginRequest) -> Response {
    // Checked before anything else touches the store.
    if request.username.trim().is_empty() || request.password.trim().is_empty() {
        return invalid_request("Username and password are required.").into_response();
    }

    let Some(user) = runtime
        .tokens()
        .validate_credentials(&request.username, &request.password)
    else {
        tracing::warn!(username = %request.username, "failed login attempt");
        // `Results.Unauthorized()` from the handler, so no challenge header —
        // that comes only from the authentication middleware.
        return unauthorized();
    };

    tracing::info!(user_id = %user.user_id, "successful login");

    match runtime.tokens().issue(&user) {
        Ok(pair) => Json(TokenResponse::from_pair(pair)).into_response(),
        Err(error) => {
            tracing::error!(%error, "could not issue a token");
            ProblemDetails::internal_server_error(new_trace_id()).into_response()
        }
    }
}

fn handle_refresh(runtime: &IdentityRuntime, request: RefreshRequest) -> Response {
    if request.user_id.trim().is_empty() || request.refresh_token.trim().is_empty() {
        return invalid_request("UserId and refreshToken are required.").into_response();
    }

    match runtime
        .tokens()
        .refresh(&request.user_id, &request.refresh_token)
    {
        Some(pair) => Json(TokenResponse::from_pair(pair)).into_response(),
        None => {
            tracing::warn!(user_id = %request.user_id, "failed refresh attempt");
            unauthorized()
        }
    }
}

fn handle_logout(
    runtime: &IdentityRuntime,
    principal: &Principal,
    request: LogoutRequest,
) -> Response {
    let user = match principal.authorize(&[Requirement::Authenticated]) {
        Ok(user) => user,
        Err(failure) => return failure.into_response(),
    };

    // A caller may only revoke their own session.
    if user.subject != request.user_id {
        tracing::warn!(
            authenticated = %user.subject,
            requested = %request.user_id,
            "logout ownership mismatch"
        );
        return AuthorizationFailure::Forbidden.into_response();
    }

    runtime
        .tokens()
        .revoke(&request.user_id, &request.refresh_token);
    tracing::info!(user_id = %request.user_id, "user logged out");

    StatusCode::NO_CONTENT.into_response()
}

/// The `userinfo` payload. Lowercase members, from an anonymous object.
#[derive(Debug, Clone, Serialize)]
struct UserInfoResponse {
    sub: String,
    name: Option<String>,
    email: Option<String>,
    roles: Vec<String>,
    permissions: Vec<String>,
}

async fn handle_userinfo(principal: Principal) -> Response {
    match principal.authorize(&[Requirement::Authenticated]) {
        Ok(user) => Json(UserInfoResponse {
            sub: user.subject.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            roles: user.roles.clone(),
            permissions: user.permissions.clone(),
        })
        .into_response(),
        Err(failure) => failure.into_response(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::TimeZone;

    #[test]
    fn the_expiry_renders_as_a_dotnet_datetimeoffset() {
        // Captured from the running service: `2026-09-11T23:09:46.900751+00:00`
        // — six digits, because the seventh was a trailing zero.
        let instant = chrono::Utc
            .with_ymd_and_hms(2026, 9, 11, 23, 9, 46)
            .unwrap()
            + chrono::Duration::nanoseconds(900_751_000);

        assert_eq!(format_offset(instant), "2026-09-11T23:09:46.900751+00:00");
    }

    #[test]
    fn a_whole_second_carries_no_fraction() {
        let instant = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();

        assert_eq!(format_offset(instant), "2026-01-02T03:04:05+00:00");
    }

    #[test]
    fn the_offset_is_written_rather_than_a_zulu_suffix() {
        // The health endpoints use `Z`; this one does not, because it renders a
        // DateTimeOffset rather than a DateTime.
        let instant = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();

        assert!(format_offset(instant).ends_with("+00:00"));
        assert!(!format_offset(instant).ends_with('Z'));
    }

    #[test]
    fn the_token_envelope_uses_snake_case_members() {
        let response = TokenResponse {
            access_token: "header.payload.signature".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_at_utc: "2026-09-11T23:09:46.900751+00:00".to_owned(),
            refresh_token: "opaque".to_owned(),
        };

        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            serde_json::json!({
                "access_token": "header.payload.signature",
                "token_type": "Bearer",
                "expires_at_utc": "2026-09-11T23:09:46.900751+00:00",
                "refresh_token": "opaque"
            })
        );
    }

    #[test]
    fn the_invalid_request_problem_matches_the_original() {
        let problem = invalid_request("Username and password are required.");
        let document = serde_json::to_value(&problem).unwrap();

        assert_eq!(document["status"], 400);
        assert_eq!(document["title"], "Invalid request");
        assert_eq!(document["detail"], "Username and password are required.");
        assert_eq!(
            document["type"], "https://tools.ietf.org/html/rfc9110#section-15.5.1",
            "Results.Problem without a type takes the framework's default"
        );
    }
}
