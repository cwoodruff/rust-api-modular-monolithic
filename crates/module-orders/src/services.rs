//! The service layer, ported from `Orders.Modules.Services`.
//!
//! Same cache-aside shape as every other module's services: compose a key,
//! read through the cache, fall back to the repository, twenty-minute
//! lifetime.

use shared_kernel::caching::CacheEntryOptions;
use shared_kernel::caching::tags::orders;
use shared_persistence::AppState;
use shared_persistence::api_models::{InvoiceApiModel, InvoiceLineApiModel};
use shared_persistence::convert::convert_all;
use shared_persistence::entities::InvoiceLine;
use shared_persistence::repositories::RepositoryResult;

/// The cache module segment these services compose keys under.
const MODULE: &str = "orders";

/// The schema version every key carries.
const VERSION: &str = "v1";

const INVOICE_TAGS: [&str; 2] = [orders::INVOICE, "orders:invoice:by-id"];
const INVOICE_LINE_TAGS: [&str; 2] = [orders::INVOICE_LINE, "orders:invoiceline:by-id"];

fn entry_options(tags: [&str; 2]) -> CacheEntryOptions {
    CacheEntryOptions::for_service(tags)
}

/// Port of `InvoiceService.GetInvoiceByIdAsync`.
pub(crate) async fn invoice_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<InvoiceApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "invoice", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.invoices.get_by_id(id).await },
            Some(entry_options(INVOICE_TAGS)),
        )
        .await
}

/// Port of `InvoiceService.GetAllInvoicesAsync`.
pub(crate) async fn all_invoices(state: &AppState) -> RepositoryResult<Vec<InvoiceApiModel>> {
    let key = state.cache_keys.compose(MODULE, "invoice", VERSION, "all");

    let invoices = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.invoices.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(INVOICE_TAGS)),
        )
        .await?;

    Ok(invoices.unwrap_or_default())
}

/// Port of `InvoiceService.GetInvoicesByCustomerIdAsync`.
pub(crate) async fn invoices_by_customer(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<InvoiceApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "invoice", VERSION, &format!("by-customer:{id}"));

    let invoices = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.invoices.get_by_customer_id(id).await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(INVOICE_TAGS)),
        )
        .await?;

    Ok(invoices.unwrap_or_default())
}

/// Port of `InvoiceLineService.GetInvoiceLineByIdAsync`.
///
/// Returns the **entity**, not an API model. The C# repository's `GetById`
/// does, and the service passes it straight through, which makes this the one
/// endpoint in the application that serves an entity shape.
pub(crate) async fn invoice_line_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<InvoiceLine>> {
    let key = state
        .cache_keys
        .compose(MODULE, "invoiceline", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.invoice_lines.get_by_id(id).await },
            Some(entry_options(INVOICE_LINE_TAGS)),
        )
        .await
}

/// Port of `InvoiceLineService.GetAllInvoiceLinesAsync`.
pub(crate) async fn all_invoice_lines(
    state: &AppState,
) -> RepositoryResult<Vec<InvoiceLineApiModel>> {
    lines_cached(state, "all", || async {
        state.repositories.invoice_lines.get_all().await
    })
    .await
}

/// Port of `InvoiceLineService.GetInvoiceLinesByInvoiceIdAsync`.
pub(crate) async fn invoice_lines_by_invoice(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<InvoiceLineApiModel>> {
    lines_cached(state, &format!("by-invoice:{id}"), || async {
        state.repositories.invoice_lines.get_by_invoice_id(id).await
    })
    .await
}

/// Port of `InvoiceLineService.GetInvoiceLinesByTrackIdAsync`.
pub(crate) async fn invoice_lines_by_track(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<InvoiceLineApiModel>> {
    lines_cached(state, &format!("by-track:{id}"), || async {
        state.repositories.invoice_lines.get_by_track_id(id).await
    })
    .await
}

/// The shape the three invoice-line collection reads share.
async fn lines_cached<F, Fut>(
    state: &AppState,
    discriminator: &str,
    fetch: F,
) -> RepositoryResult<Vec<InvoiceLineApiModel>>
where
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = RepositoryResult<Vec<InvoiceLine>>> + Send,
{
    let key = state
        .cache_keys
        .compose(MODULE, "invoiceline", VERSION, discriminator);

    let lines = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = fetch().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(INVOICE_LINE_TAGS)),
        )
        .await?;

    Ok(lines.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use shared_kernel::caching::CacheKeyComposer;

    #[test]
    fn the_cache_keys_match_the_originals() {
        let composer = CacheKeyComposer::from_parts("mmapi", "test");

        assert_eq!(
            composer
                .compose(MODULE, "invoice", VERSION, "by-customer:1")
                .to_string(),
            "test:mmapi:orders:invoice:v1::::by-customer:1"
        );
        assert_eq!(
            composer
                .compose(MODULE, "invoiceline", VERSION, "all")
                .to_string(),
            "test:mmapi:orders:invoiceline:v1::::all",
            "the entity segment is `invoiceline`, not `invoice-line`"
        );
    }

    #[test]
    fn every_entity_carries_its_two_tags() {
        assert_eq!(INVOICE_TAGS, ["orders:invoice", "orders:invoice:by-id"]);
        assert_eq!(
            INVOICE_LINE_TAGS,
            ["orders:invoiceline", "orders:invoiceline:by-id"]
        );
    }
}
