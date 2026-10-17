//! Catalog's reaction to `InvoiceFinalized`: units sold per track (ADR-0009).

use orders_contracts::events::InvoiceFinalized;
use shared_kernel::events::{HandlerError, IntegrationEventHandler};
use shared_kernel::hosting::BoxFuture;
use shared_kernel::persistence::DbConnection;

/// Port of `Catalog.Modules.Events.InvoiceFinalizedHandler`.
///
/// For each track on the invoice: add the line quantities to `TimesSold`, and
/// keep the latest invoice date as `LastSoldAt`. Writes go through the inbox
/// guard's transaction and are committed by it, with the inbox row.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InvoiceFinalizedHandler;

impl IntegrationEventHandler<InvoiceFinalized> for InvoiceFinalizedHandler {
    const HANDLER_NAME: &'static str = "Catalog.Modules.Events.InvoiceFinalizedHandler";

    fn handle<'a>(
        &'a self,
        event: &'a InvoiceFinalized,
        connection: &'a mut DbConnection,
    ) -> BoxFuture<'a, Result<(), HandlerError>> {
        Box::pin(async move {
            // Grouped by track, as upstream groups the lines.
            let mut per_track: Vec<(i32, i32)> = Vec::new();
            for line in &event.lines {
                match per_track
                    .iter_mut()
                    .find(|(track, _)| *track == line.track_id)
                {
                    Some((_, quantity)) => *quantity += line.quantity,
                    None => per_track.push((line.track_id, line.quantity)),
                }
            }

            for (track_id, quantity) in per_track {
                sqlx::query(
                    r#"INSERT INTO catalog."TrackSales" ("TrackId", "TimesSold", "LastSoldAt")
                       VALUES ($1, $2, $3)
                       ON CONFLICT ("TrackId") DO UPDATE SET
                           "TimesSold" = catalog."TrackSales"."TimesSold" + EXCLUDED."TimesSold",
                           "LastSoldAt" = CASE
                               WHEN catalog."TrackSales"."LastSoldAt" IS NULL
                                 OR EXCLUDED."LastSoldAt" > catalog."TrackSales"."LastSoldAt"
                               THEN EXCLUDED."LastSoldAt"
                               ELSE catalog."TrackSales"."LastSoldAt" END"#,
                )
                .bind(track_id)
                .bind(quantity)
                .bind(event.invoice_date)
                .execute(&mut *connection)
                .await?;
            }

            Ok(())
        })
    }
}
