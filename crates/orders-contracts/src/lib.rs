//! Port of `Orders.Contracts`: what Orders publishes.

pub mod events {
    //! Integration events, serialized camelCase as upstream's outbox writes them.

    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use serde::{Deserialize, Serialize};
    use shared_kernel::events::IntegrationEvent;
    use uuid::Uuid;

    /// An invoice became final. Catalog counts track sales from it; Administration
    /// sums the customer's purchases (ADR-0009, ADR-0010).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InvoiceFinalized {
        /// The event's identity and its outbox row's key.
        pub event_id: Uuid,
        /// When the invoice was finalized.
        pub occurred_at: DateTime<Utc>,
        /// The invoice.
        pub invoice_id: i32,
        /// The customer, an id only: Orders does not own customers (ADR-0004).
        pub customer_id: Option<i32>,
        /// The invoice's own date, not the finalization time.
        pub invoice_date: DateTime<Utc>,
        /// The invoice total.
        pub total: Decimal,
        /// One entry per invoice line that names a track.
        pub lines: Vec<InvoiceFinalizedLine>,
    }

    /// One line of a finalized invoice.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InvoiceFinalizedLine {
        /// The track, an id only (ADR-0005).
        pub track_id: i32,
        /// Units sold.
        pub quantity: i32,
        /// Price per unit.
        pub unit_price: Decimal,
    }

    impl IntegrationEvent for InvoiceFinalized {
        const EVENT_TYPE: &'static str = "Orders.Contracts.Events.InvoiceFinalized";

        fn event_id(&self) -> Uuid {
            self.event_id
        }

        fn occurred_at(&self) -> DateTime<Utc> {
            self.occurred_at
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used)]

        use super::*;

        #[test]
        fn the_payload_is_camel_case_as_upstreams_outbox_writes_it() {
            let event = InvoiceFinalized {
                event_id: Uuid::nil(),
                occurred_at: DateTime::UNIX_EPOCH,
                invoice_id: 1,
                customer_id: Some(2),
                invoice_date: DateTime::UNIX_EPOCH,
                total: Decimal::new(198, 2),
                lines: vec![InvoiceFinalizedLine {
                    track_id: 2,
                    quantity: 1,
                    unit_price: Decimal::new(99, 2),
                }],
            };

            let json = serde_json::to_value(&event).unwrap();

            assert_eq!(json["invoiceId"], 1);
            assert_eq!(json["customerId"], 2);
            assert_eq!(json["total"], 1.98);
            assert_eq!(json["lines"][0]["trackId"], 2);
            assert_eq!(json["lines"][0]["unitPrice"], 0.99);
        }
    }
}
