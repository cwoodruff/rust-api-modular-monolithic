//! Administration's reaction to `InvoiceFinalized`: what each customer has
//! spent (ADR-0010).

use orders_contracts::events::InvoiceFinalized;
use shared_kernel::events::{HandlerError, IntegrationEventHandler};
use shared_kernel::hosting::BoxFuture;
use shared_kernel::persistence::DbConnection;

/// Port of `Admin.Modules.Events.InvoiceFinalizedHandler`.
///
/// An event with no customer is ignored — the inbox row still commits. There
/// is no check that the customer exists: the id is application-validated, and
/// the orphan check reports what does not resolve (ADR-0004, ADR-0015).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InvoiceFinalizedHandler;

impl IntegrationEventHandler<InvoiceFinalized> for InvoiceFinalizedHandler {
    const HANDLER_NAME: &'static str = "Admin.Modules.Events.InvoiceFinalizedHandler";

    fn handle<'a>(
        &'a self,
        event: &'a InvoiceFinalized,
        connection: &'a mut DbConnection,
    ) -> BoxFuture<'a, Result<(), HandlerError>> {
        Box::pin(async move {
            let Some(customer_id) = event.customer_id else {
                return Ok(());
            };

            sqlx::query(
                r#"INSERT INTO administration."CustomerPurchaseSummary" ("CustomerId", "TotalSpent", "InvoiceCount", "LastPurchaseAt")
                   VALUES ($1, $2, 1, $3)
                   ON CONFLICT ("CustomerId") DO UPDATE SET
                       "TotalSpent" = administration."CustomerPurchaseSummary"."TotalSpent" + EXCLUDED."TotalSpent",
                       "InvoiceCount" = administration."CustomerPurchaseSummary"."InvoiceCount" + 1,
                       "LastPurchaseAt" = CASE
                           WHEN administration."CustomerPurchaseSummary"."LastPurchaseAt" IS NULL
                             OR EXCLUDED."LastPurchaseAt" > administration."CustomerPurchaseSummary"."LastPurchaseAt"
                           THEN EXCLUDED."LastPurchaseAt"
                           ELSE administration."CustomerPurchaseSummary"."LastPurchaseAt" END"#,
            )
            .bind(customer_id)
            .bind(event.total)
            .bind(event.invoice_date)
            .execute(&mut *connection)
            .await?;

            Ok(())
        })
    }
}
