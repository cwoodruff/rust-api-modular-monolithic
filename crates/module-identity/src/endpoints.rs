//! The five auth endpoints, ported from `AuthEndpoints`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use shared_kernel::auth::{Caller, unauthorized};
use shared_kernel::diagnostics::ModuleMeter;
use shared_kernel::endpoints::Endpoints;
use shared_kernel::errors::current_trace_id;
use shared_kernel::traffic_control::RateLimiter;
use shared_kernel::validation::{Validated, ValidationErrors, required};
use shared_kernel::{ApiError, AppConfig, DatabaseProbe, ProblemDetails, REDACTED, health};

use crate::runtime::IdentityRuntime;
use crate::tokens::TokenPair;

/// The login request body. Member names are lowercase, as the record declares.
///
/// A rejected login is logged, and the natural thing to log is the request.
/// `Debug` therefore keeps the username, which is what makes the record useful,
/// and drops the password, which is what makes it dangerous.
#[derive(Clone, Deserialize)]
pub struct LoginRequest {
    /// The login name.
    #[serde(default, alias = "Username", alias = "UserName")]
    pub username: Option<String>,
    /// The password.
    #[serde(default, alias = "Password")]
    pub password: Option<String>,
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &REDACTED)
            .finish()
    }
}

/// The refresh request body.
#[derive(Clone, Deserialize)]
pub struct RefreshRequest {
    /// The user whose token is being exchanged.
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    pub user_id: Option<String>,
    /// The refresh token.
    #[serde(
        default,
        rename = "refreshToken",
        alias = "RefreshToken",
        alias = "refreshtoken"
    )]
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for RefreshRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RefreshRequest")
            .field("user_id", &self.user_id)
            .field("refresh_token", &REDACTED)
            .finish()
    }
}

/// The logout request body.
#[derive(Clone, Deserialize)]
pub struct LogoutRequest {
    /// The user whose token is being revoked.
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    pub user_id: Option<String>,
    /// The refresh token to revoke.
    #[serde(
        default,
        rename = "refreshToken",
        alias = "RefreshToken",
        alias = "refreshtoken"
    )]
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for LogoutRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogoutRequest")
            .field("user_id", &self.user_id)
            .field("refresh_token", &REDACTED)
            .finish()
    }
}

/// The token envelope.
///
/// Snake-case members, unlike the PascalCase API models — these come from a C#
/// anonymous object and the host's naming policy leaves both alone.
#[derive(Clone, Serialize)]
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

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokenResponse")
            .field("access_token", &REDACTED)
            .field("token_type", &self.token_type)
            .field("expires_at_utc", &self.expires_at_utc)
            .field("refresh_token", &REDACTED)
            .finish()
    }
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
    shared_kernel::json::dotnet::date_time_offset(&instant)
}

/// The body validators, ported from `AuthRequestValidators` (18f7f86).
///
/// Every field is required: a blank or missing one is `'<field>' is required.`
/// under its wire name, in declaration order.
pub(crate) fn validate_login(request: &LoginRequest) -> ValidationErrors {
    required(&[
        ("username", request.username.as_deref()),
        ("password", request.password.as_deref()),
    ])
}

/// The refresh body's rules.
pub(crate) fn validate_refresh(request: &RefreshRequest) -> ValidationErrors {
    required(&[
        ("userId", request.user_id.as_deref()),
        ("refreshToken", request.refresh_token.as_deref()),
    ])
}

/// The logout body's rules.
pub(crate) fn validate_logout(request: &LogoutRequest) -> ValidationErrors {
    required(&[
        ("userId", request.user_id.as_deref()),
        ("refreshToken", request.refresh_token.as_deref()),
    ])
}

type Identity = State<Arc<IdentityState>>;

/// Everything Identity's handlers read.
pub(crate) struct IdentityState {
    pub runtime: Arc<IdentityRuntime>,
    pub rate_limiter: RateLimiter,
    pub meter: Arc<ModuleMeter>,
    pub config: Arc<AppConfig>,
}

/// Maps the auth endpoints.
///
/// Note where JWKS lands: inside the module group, so its real path is
/// `/api/identity/.well-known/jwks.json` rather than the conventional root
/// one. That is the original's arrangement and it is wire-visible (P3).
pub(crate) fn map(endpoints: &mut Endpoints, state: &Arc<IdentityState>) {
    let mut group = endpoints.map_group(crate::PREFIX, crate::TAG, Arc::clone(state));
    group
        .require_rate_limiting(state.rate_limiter.clone())
        .add_module_metrics(Arc::clone(&state.meter));

    health::map_health_endpoints(
        &mut group,
        crate::NAME,
        Arc::new(AppDatabaseProbe(Arc::clone(&state.config))),
    );

    group
        .map_post("/login", login)
        .with_name("IdentityLogin")
        .with_validation::<LoginRequest>(Arc::new(validate_login))
        .produces(&[200, 400, 401, 429]);
    group
        .map_post("/refresh", refresh)
        .with_name("IdentityRefresh")
        .with_validation::<RefreshRequest>(Arc::new(validate_refresh))
        .produces(&[200, 400, 401, 429]);
    group
        .map_post("/logout", logout)
        .with_name("IdentityLogout")
        .require_authorization(&[])
        .with_validation::<LogoutRequest>(Arc::new(validate_logout))
        .produces(&[204, 400, 401, 403, 429]);
    group
        .map_get("/userinfo", userinfo)
        .with_name("IdentityUserInfo")
        .require_authorization(&[])
        .produces(&[200, 401]);
    group
        .map_get("/.well-known/jwks.json", jwks)
        .with_name("IdentityJWKS")
        .produces(&[200]);
}

/// Identity has no tables: its data-health asks whether `AppDatabase` answers.
struct AppDatabaseProbe(Arc<AppConfig>);

#[async_trait::async_trait]
impl DatabaseProbe for AppDatabaseProbe {
    async fn can_connect(&self) -> bool {
        shared_kernel::persistence::can_connect_with(&self.0).await
    }
}

/// Port of `AuthHandlers.Login`.
pub(crate) async fn login(
    State(identity): Identity,
    Validated(request): Validated<LoginRequest>,
) -> Response {
    let username = request.username.unwrap_or_default();
    let password = request.password.unwrap_or_default();

    let Some(user) = identity
        .runtime
        .tokens()
        .validate_credentials(&username, &password)
    else {
        tracing::warn!("Failed login attempt for user '{username}'");
        // `Results.Unauthorized()` from the handler, so no challenge header —
        // that comes only from the authentication middleware.
        return unauthorized();
    };

    tracing::info!("Successful login for user '{}'", user.user_id);

    match identity.runtime.tokens().issue(&user) {
        Ok(pair) => Json(TokenResponse::from_pair(pair)).into_response(),
        Err(error) => {
            tracing::error!(%error, "could not issue a token");
            ProblemDetails::internal_server_error(current_trace_id()).into_response()
        }
    }
}

/// Port of `AuthHandlers.Refresh`.
pub(crate) async fn refresh(
    State(identity): Identity,
    Validated(request): Validated<RefreshRequest>,
) -> Response {
    let user_id = request.user_id.unwrap_or_default();
    let refresh_token = request.refresh_token.unwrap_or_default();

    match identity.runtime.tokens().refresh(&user_id, &refresh_token) {
        Some(pair) => Json(TokenResponse::from_pair(pair)).into_response(),
        None => {
            tracing::warn!("Failed refresh attempt for user '{user_id}'");
            unauthorized()
        }
    }
}

/// Port of `AuthHandlers.Logout`.
pub(crate) async fn logout(
    State(identity): Identity,
    Caller(caller): Caller,
    Validated(request): Validated<LogoutRequest>,
) -> Result<StatusCode, ApiError> {
    let user_id = request.user_id.unwrap_or_default();

    // A caller may only revoke their own session.
    if caller.subject != user_id {
        tracing::warn!(
            "Logout ownership mismatch: authenticated '{}' tried to revoke tokens for '{user_id}'",
            caller.subject
        );
        return Err(ApiError::Forbidden);
    }

    identity
        .runtime
        .tokens()
        .revoke(&user_id, &request.refresh_token.unwrap_or_default());
    tracing::info!("User '{user_id}' logged out");

    Ok(StatusCode::NO_CONTENT)
}

/// The `userinfo` payload. Lowercase members, upstream's `[JsonPropertyName]`s.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UserInfoResponse {
    sub: String,
    name: Option<String>,
    email: Option<String>,
    roles: Vec<String>,
    permissions: Vec<String>,
}

/// Port of `AuthHandlers.UserInfo`.
pub(crate) async fn userinfo(Caller(caller): Caller) -> Json<UserInfoResponse> {
    Json(UserInfoResponse {
        sub: caller.subject,
        name: caller.name,
        email: caller.email,
        roles: caller.roles,
        permissions: caller.permissions,
    })
}

/// Port of `AuthHandlers.Jwks`.
pub(crate) async fn jwks(State(identity): Identity) -> Json<serde_json::Value> {
    Json(identity.runtime.tokens().jwks())
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
    fn a_failed_login_can_be_logged_without_logging_the_password() {
        // The handler warns on a rejected login, and `?request` is the obvious
        // thing to put in that record.
        let request = LoginRequest {
            username: Some("demo".to_owned()),
            password: Some("hunter2".to_owned()),
        };

        let rendered = format!("{request:?}");

        assert!(rendered.contains("demo"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }

    #[test]
    fn neither_token_body_prints_the_token_it_carries() {
        let refresh = RefreshRequest {
            user_id: Some("user-1".to_owned()),
            refresh_token: Some("an-opaque-credential".to_owned()),
        };
        let logout = LogoutRequest {
            user_id: Some("user-1".to_owned()),
            refresh_token: Some("an-opaque-credential".to_owned()),
        };
        let response = TokenResponse {
            access_token: "header.payload.signature".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_at_utc: "2026-09-11T23:09:46.900751+00:00".to_owned(),
            refresh_token: "an-opaque-credential".to_owned(),
        };

        for rendered in [
            format!("{refresh:?}"),
            format!("{logout:?}"),
            format!("{response:?}"),
        ] {
            assert!(!rendered.contains("an-opaque-credential"), "{rendered}");
            assert!(!rendered.contains("header.payload.signature"), "{rendered}");
            assert!(
                rendered.contains("user-1") || rendered.contains("Bearer"),
                "{rendered}"
            );
        }
    }

    #[test]
    fn a_blank_field_is_required_under_its_wire_name() {
        let errors = validate_login(&LoginRequest {
            username: Some("demo".to_owned()),
            password: Some("   ".to_owned()),
        });

        assert_eq!(errors.len(), 1);
        assert_eq!(errors["password"], ["'password' is required."]);

        let errors = validate_logout(&LogoutRequest {
            user_id: None,
            refresh_token: None,
        });
        assert_eq!(
            errors.keys().collect::<Vec<_>>(),
            ["refreshToken", "userId"]
        );
    }

    #[test]
    fn bodies_bind_case_insensitively_as_upstreams_do() {
        let request: LoginRequest =
            serde_json::from_str(r#"{"Username":"demo","Password":"pw"}"#).unwrap();

        assert_eq!(request.username.as_deref(), Some("demo"));
    }
}
