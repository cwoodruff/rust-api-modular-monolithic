//! Reporting's routes, ported from `Reporting.Module/Endpoints`.
//!
//! One group at `/api/reporting` with the module's own rate limit and metrics.
//! The two reads need `report.view` and `tenant.scoped`; the integrity routes
//! need `role.admin` only.

use std::sync::Arc;

use identity_contracts::{permissions, policies};
use shared_kernel::endpoints::Endpoints;
use shared_kernel::health;

use crate::{NAME, PREFIX, ReportingState, TAG, handlers};

const VIEW: &[&str] = &[permissions::REPORT_VIEW, policies::TENANT_SCOPED];
const ADMIN: &[&str] = &[policies::ADMIN];

pub(crate) fn map(endpoints: &mut Endpoints, state: &Arc<ReportingState>) {
    let mut group = endpoints.map_group(PREFIX, TAG, Arc::clone(state));
    group
        .require_rate_limiting(state.rate_limiter.clone())
        .add_module_metrics(Arc::clone(&state.meter));

    // Data-health connects as the reporting login, so a missing login reads Degraded.
    health::map_health_endpoints(&mut group, NAME, Arc::new(state.database.clone()));

    group
        .map_get("/sales-by-genre", handlers::sales_by_genre)
        .with_name("ReportingSalesByGenre")
        .with_description(
            "Units sold per genre, from Catalog's TrackSales (eventually consistent, ADR-0009).",
        )
        .require_authorization(VIEW)
        .produces(&[200, 401, 403, 429]);

    group
        .map_get("/invoices/{id:int}/lines", handlers::invoice_lines)
        .with_name("ReportingInvoiceLines")
        .with_description(
            "An invoice's lines with track and customer names, read across modules through a view.",
        )
        .require_authorization(VIEW)
        .produces(&[200, 401, 403, 404, 429]);

    group
        .map_post("/integrity/run", handlers::run_integrity_check)
        .with_name("ReportingRunIntegrityCheck")
        .with_description("Runs the orphan checks now and returns how many findings were added, resolved and left open.")
        .require_authorization(ADMIN)
        .produces(&[200, 401, 403, 429]);

    group
        .map_get("/integrity/findings", handlers::integrity_findings)
        .with_name("ReportingIntegrityFindings")
        .with_description("Open findings: rows whose cross-module reference points at nothing. Nothing is repaired.")
        .require_authorization(ADMIN)
        .produces(&[200, 401, 403, 429]);
}
