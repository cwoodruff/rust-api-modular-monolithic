//! Health endpoints, ported from the five modules' `HealthEndpoints` and
//! `DataHealthEndpoints`.
//!
//! Every module in the original carries its own near-identical copy of these
//! two handlers — ten files differing only in a module name. Here the shape and
//! the handlers are defined once and each module mounts them under its own
//! name, which is the same division the original would have reached for had the
//! duplication ever been collapsed.
//!
//! Three properties are wire-visible and reproduced exactly:
//!
//! 1. **The keys are lowercase.** These come from C# anonymous objects, not the
//!    PascalCase API models, and the host's naming policy leaves both alone.
//! 2. **Outside Development and Demo the extra members are absent**, not null.
//!    The original returns a genuinely different anonymous type there, so those
//!    keys do not appear at all.
//! 3. **Both endpoints always answer 200**, including when the database is
//!    unreachable and `status` reads `Degraded`. Neither is usable as a liveness
//!    probe without parsing the body.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::build_info;
use crate::environment::Environment;

/// `status` when the module itself is up.
pub const HEALTHY: &str = "Healthy";

/// `status` when the database answered.
pub const DATA_HEALTHY: &str = "Data-Healthy";

/// `status` when the database did not.
pub const DEGRADED: &str = "Degraded";

/// The `module` value the root endpoint reports.
pub const ROOT_MODULE: &str = "root";

/// Answers whether the database is reachable.
///
/// Port of the `db.Database.CanConnectAsync(ct)` call the five `/data-health`
/// endpoints make. A trait so that neither this crate nor a module crate needs
/// to see a database driver.
#[async_trait]
pub trait DatabaseProbe: Send + Sync {
    /// Whether a connection can be opened and used.
    ///
    /// Never fails: the original wraps the call in a bare `catch` and reports
    /// `false`, so a probe that throws and a probe that answers "no" look the
    /// same to a caller.
    async fn can_connect(&self) -> bool;
}

/// What the health handlers need from the host's state.
///
/// Implemented by the application state so this crate can build the routes
/// without depending on the crate that owns the repositories.
pub trait HealthContext: Clone + Send + Sync + 'static {
    /// The environment the host is running as.
    fn environment(&self) -> &Environment;

    /// The configured service name.
    fn service_name(&self) -> String;

    /// The database probe backing `/data-health`.
    fn database_probe(&self) -> Arc<dyn DatabaseProbe>;
}

/// A health or data-health response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// The reporting module, or `root`.
    pub module: String,

    /// `Healthy`, `Data-Healthy`, or `Degraded`.
    pub status: String,

    /// When the response was produced, in C#'s round-trip format.
    pub timestamp_utc: String,

    /// The environment name. Development and Demo only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,

    /// The build version. Development and Demo only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// The configured service name. Development and Demo only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,

    /// Database connectivity. Data-health in Development and Demo only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseStatus>,
}

/// The nested `database` member of a data-health response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseStatus {
    /// Whether the database answered.
    pub connected: bool,
}

impl HealthResponse {
    /// The minimal response, as every environment outside Development and Demo
    /// receives it.
    #[must_use]
    pub fn new(module: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            status: status.into(),
            timestamp_utc: timestamp_utc(),
            environment: None,
            version: None,
            service: None,
            database: None,
        }
    }

    /// Adds the members Development and Demo expose.
    #[must_use]
    pub fn with_metadata(
        mut self,
        environment: impl Into<String>,
        version: impl Into<String>,
        service: impl Into<String>,
    ) -> Self {
        self.environment = Some(environment.into());
        self.version = Some(version.into());
        self.service = Some(service.into());
        self
    }

    /// Adds the `database` member, which data-health exposes alongside the rest
    /// of the metadata.
    #[must_use]
    pub fn with_database(mut self, connected: bool) -> Self {
        self.database = Some(DatabaseStatus { connected });
        self
    }

    /// Applies the metadata only where the environment exposes it.
    #[must_use]
    pub fn gated<S: HealthContext>(self, state: &S) -> Self {
        let environment = state.environment();

        if environment.exposes_operational_metadata() {
            self.with_metadata(
                environment.name(),
                build_info::version(),
                state.service_name(),
            )
        } else {
            self
        }
    }
}

/// Mounts `/health` and `/data-health` for one module.
///
/// Port of `group.MapXHealthEndpoints()` and `group.MapXDataHealthEndpoints()`.
/// Both stay anonymous — the original leaves every health endpoint
/// unauthenticated, including the ones in otherwise protected modules.
pub fn routes<S: HealthContext>(module: &'static str) -> Router<S> {
    Router::new()
        .route(
            "/health",
            get(move |State(state): State<S>| async move {
                Json(HealthResponse::new(module, HEALTHY).gated(&state))
            }),
        )
        .route(
            "/data-health",
            get(move |State(state): State<S>| async move {
                let connected = state.database_probe().can_connect().await;
                let status = if connected { DATA_HEALTHY } else { DEGRADED };

                let mut response = HealthResponse::new(module, status).gated(&state);

                // The database member rides along with the rest of the
                // metadata, so it is absent outside Development and Demo.
                if state.environment().exposes_operational_metadata() {
                    response = response.with_database(connected);
                }

                Json(response)
            }),
        )
}

/// Renders the current instant the way `DateTime.UtcNow.ToString("O")` does.
///
/// C#'s round-trip format for a UTC `DateTime` is
/// `yyyy-MM-ddTHH:mm:ss.fffffffZ` — seven fractional digits, because .NET
/// counts 100-nanosecond ticks. `chrono` offers three or nine, so the tick
/// count is built explicitly.
#[must_use]
pub fn timestamp_utc() -> String {
    format_timestamp(chrono::Utc::now())
}

fn format_timestamp(instant: chrono::DateTime<chrono::Utc>) -> String {
    let ticks = instant.timestamp_subsec_nanos() / 100;
    format!("{}.{ticks:07}Z", instant.format("%Y-%m-%dT%H:%M:%S"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn the_minimal_response_carries_only_three_members() {
        let mut response = HealthResponse::new("Music", HEALTHY);
        response.timestamp_utc = "2026-09-11T14:23:45.1234567Z".to_owned();

        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "module": "Music",
                "status": "Healthy",
                "timestampUtc": "2026-09-11T14:23:45.1234567Z"
            }),
            "outside Development and Demo the extra keys are absent, not null"
        );
    }

    #[test]
    fn the_development_response_adds_the_operational_members() {
        let mut response = HealthResponse::new("Music", HEALTHY).with_metadata(
            "Development",
            "1.0.0",
            "ModularMonolith.Api",
        );
        response.timestamp_utc = "2026-09-11T14:23:45.1234567Z".to_owned();

        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "module": "Music",
                "status": "Healthy",
                "timestampUtc": "2026-09-11T14:23:45.1234567Z",
                "environment": "Development",
                "version": "1.0.0",
                "service": "ModularMonolith.Api"
            })
        );
    }

    #[test]
    fn a_data_health_response_nests_the_database_member() {
        let response = HealthResponse::new("Orders", DATA_HEALTHY)
            .with_metadata("Demo", "1.0.0", "svc")
            .with_database(true);

        let document = serde_json::to_value(&response).unwrap();

        assert_eq!(document["status"], "Data-Healthy");
        assert_eq!(document["database"], json!({ "connected": true }));
    }

    #[test]
    fn an_unreachable_database_reports_degraded_rather_than_failing() {
        let response = HealthResponse::new("Orders", DEGRADED).with_database(false);

        let document = serde_json::to_value(&response).unwrap();

        assert_eq!(document["status"], "Degraded");
        assert_eq!(document["database"], json!({ "connected": false }));
    }

    #[test]
    fn the_timestamp_matches_the_round_trip_format() {
        let instant = chrono::Utc
            .with_ymd_and_hms(2026, 9, 11, 14, 23, 45)
            .unwrap()
            + chrono::Duration::nanoseconds(123_456_789);

        // .NET's "O" keeps seven digits — 100-nanosecond ticks — so the
        // trailing 89 nanoseconds are truncated, not rounded.
        assert_eq!(format_timestamp(instant), "2026-09-11T14:23:45.1234567Z");
    }

    #[test]
    fn the_timestamp_always_has_seven_fractional_digits() {
        let instant = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();

        assert_eq!(format_timestamp(instant), "2026-01-02T03:04:05.0000000Z");
    }

    #[test]
    fn a_generated_timestamp_has_the_right_shape() {
        let stamp = timestamp_utc();

        assert!(stamp.ends_with('Z'), "{stamp}");
        assert_eq!(
            stamp.len(),
            28,
            "{stamp} should be yyyy-MM-ddTHH:mm:ss.fffffffZ"
        );
    }
}
