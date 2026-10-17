//! Orders' routes, ported from `Orders.Module/Endpoints`.
//!
//! One group at `/api/orders`, carrying the module's own rate limit and
//! metrics. Reads require `orders.read` and `tenant.scoped`; finalizing
//! requires `orders.write` and `tenant.scoped`; the dead-letter routes require
//! `role.admin` alone. Health and data-health stay anonymous.

use std::sync::Arc;

use identity_contracts::{permissions, policies};
use shared_kernel::endpoints::Endpoints;
use shared_kernel::health;

use crate::{NAME, OrdersState, PREFIX, TAG, handlers};

const READ: &[&str] = &[permissions::ORDERS_READ, policies::TENANT_SCOPED];
const WRITE: &[&str] = &[permissions::ORDERS_WRITE, policies::TENANT_SCOPED];
const ADMIN: &[&str] = &[policies::ADMIN];

/// What a by-id route can answer.
const BY_ID: &[u16] = &[200, 401, 403, 404, 429];
/// What a collection route can answer.
const LIST: &[u16] = &[200, 401, 403, 429];

pub(crate) fn map(endpoints: &mut Endpoints, state: &Arc<OrdersState>) {
    let mut group = endpoints.map_group(PREFIX, TAG, Arc::clone(state));
    group
        .require_rate_limiting(state.rate_limiter.clone())
        .add_module_metrics(Arc::clone(&state.meter));

    health::map_health_endpoints(&mut group, NAME, Arc::new(state.database.clone()));

    // Invoices
    group
        .map_get("/invoices/{id:int}", handlers::get_invoice_by_id)
        .with_name("OrdersGetInvoiceById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/invoices/", handlers::get_all_invoices)
        .with_name("GetAllInvoices")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/invoices/customer/{id:int}",
            handlers::get_invoices_by_customer_id,
        )
        .with_name("GetInvoicesByCustomerId")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_post("/invoices/{id:int}/finalize", handlers::finalize_invoice)
        .with_name("OrdersFinalizeInvoice")
        .with_description(
            "Finalizes a draft invoice and publishes InvoiceFinalized through the orders outbox in the same transaction. Returns 202: the invoice is final now, but track sales and the customer's purchase summary are updated eventually, after the outbox is dispatched (salesCountersUpdate: \"eventual\"). Do not read them back expecting this invoice to be counted yet.",
        )
        .require_authorization(WRITE)
        .produces(&[202, 401, 403, 404, 409, 429]);

    // Invoice lines
    group
        .map_get("/invoice-lines/{id:int}", handlers::get_invoice_line_by_id)
        .with_name("OrdersGetInvoiceLineById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/invoice-lines/", handlers::get_all_invoice_lines)
        .with_name("GetAllInvoiceLines")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/invoice-lines/invoice/{id:int}",
            handlers::get_invoice_lines_by_invoice_id,
        )
        .with_name("GetInvoiceLinesByInvoiceId")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/invoice-lines/track/{id:int}",
            handlers::get_invoice_lines_by_track_id,
        )
        .with_name("GetInvoiceLinesByTrackId")
        .require_authorization(READ)
        .produces(LIST);

    // Outbox
    group
        .map_get("/outbox/dead-letters", handlers::list_dead_letters)
        .with_name("OrdersListDeadLetters")
        .with_description(
            "Outbox messages whose delivery failed six times. Each stays here until retried.",
        )
        .require_authorization(ADMIN)
        .produces(LIST);
    group
        .map_post("/outbox/dead-letters/{id:guid}/retry", handlers::retry_dead_letter)
        .with_name("OrdersRetryDeadLetter")
        .with_description("Resets the attempt count so the dispatcher delivers the message again on its next poll.")
        .require_authorization(ADMIN)
        .produces(&[204, 401, 403, 404, 429]);
}
