//! Invoice and invoice line repositories.

use async_trait::async_trait;
use shared_persistence::api_models::{CustomerApiModel, InvoiceApiModel, InvoiceLineApiModel};
use shared_persistence::entities::{Invoice, InvoiceLine};
use shared_persistence::repositories::{
    InvoiceLineRepository, InvoiceRepository, Repository, RepositoryResult,
};
use sqlx::SqlitePool;

use super::common::{self, tables};
use crate::rows;

/// Every scalar column on `Invoice`.
const INVOICE_COLUMNS: &str = r#""Id", "CustomerId", "InvoiceDate", "BillingAddress", "BillingCity",
    "BillingState", "BillingCountry", "BillingPostalCode", "Total""#;

/// Every scalar column on `InvoiceLine`.
const INVOICE_LINE_COLUMNS: &str = r#""Id", "InvoiceId", "TrackId", "UnitPrice", "Quantity""#;

/// sqlx implementation of [`InvoiceRepository`].
#[derive(Debug, Clone)]
pub struct SqliteInvoiceRepository {
    pool: SqlitePool,
}

impl SqliteInvoiceRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<Invoice> for SqliteInvoiceRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::INVOICE, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Invoice>> {
        let rows = sqlx::query(&format!(
            r#"SELECT {INVOICE_COLUMNS} FROM "Invoice" ORDER BY "Id""#
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::invoice)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Invoice) -> RepositoryResult<Invoice> {
        let row = sqlx::query(&format!(
            r#"INSERT INTO "Invoice" ("CustomerId", "InvoiceDate", "BillingAddress", "BillingCity",
                                      "BillingState", "BillingCountry", "BillingPostalCode", "Total")
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               RETURNING {INVOICE_COLUMNS}"#
        ))
        .bind(entity.customer_id)
        .bind(entity.invoice_date)
        .bind(&entity.billing_address)
        .bind(&entity.billing_city)
        .bind(&entity.billing_state)
        .bind(&entity.billing_country)
        .bind(&entity.billing_postal_code)
        .bind(rows::money(Some(entity.total)))
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;

        rows::invoice(&row).map_err(common::database)
    }

    async fn update(&self, entity: Invoice) -> RepositoryResult<bool> {
        if !self.entity_exists(entity.id).await? {
            return Ok(false);
        }

        sqlx::query(
            r#"UPDATE "Invoice" SET "CustomerId" = ?, "InvoiceDate" = ?, "BillingAddress" = ?,
                                    "BillingCity" = ?, "BillingState" = ?, "BillingCountry" = ?,
                                    "BillingPostalCode" = ?, "Total" = ?
               WHERE "Id" = ?"#,
        )
        .bind(entity.customer_id)
        .bind(entity.invoice_date)
        .bind(&entity.billing_address)
        .bind(&entity.billing_city)
        .bind(&entity.billing_state)
        .bind(&entity.billing_country)
        .bind(&entity.billing_postal_code)
        .bind(rows::money(Some(entity.total)))
        .bind(entity.id)
        .execute(&self.pool)
        .await
        .map_err(common::database)?;

        Ok(true)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::INVOICE, id).await
    }
}

#[async_trait]
impl InvoiceRepository for SqliteInvoiceRepository {
    async fn get_by_customer_id(&self, id: i32) -> RepositoryResult<Vec<Invoice>> {
        let rows = sqlx::query(&format!(
            r#"SELECT {INVOICE_COLUMNS} FROM "Invoice" WHERE "CustomerId" = ? ORDER BY "Id""#
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::invoice)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    /// The invoice with a shallow customer and its lines.
    ///
    /// The nested customer is deliberately partial in the original — it
    /// projects only key, names, company, email, phone, and the support rep's
    /// key and name, leaving address, city, state, country, postal code, and
    /// fax null. Its own `Invoices` list stays empty and `SupportRep` stays
    /// null, which is how the original avoids a cycle back to this invoice.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<InvoiceApiModel>> {
        let Some(header) = sqlx::query(
            r#"SELECT i."Id", i."CustomerId", i."InvoiceDate", i."BillingAddress", i."BillingCity",
                      i."BillingState", i."BillingCountry", i."BillingPostalCode", i."Total",
                      c."Id" AS "CustomerKey", c."FirstName" AS "CustomerFirstName",
                      c."LastName" AS "CustomerLastName", c."Company" AS "CustomerCompany",
                      c."Email" AS "CustomerEmail", c."Phone" AS "CustomerPhone",
                      c."SupportRepId" AS "CustomerSupportRepId",
                      rep."FirstName" AS "RepFirstName", rep."LastName" AS "RepLastName"
               FROM "Invoice" i
               LEFT JOIN "Customer" c ON c."Id" = i."CustomerId"
               LEFT JOIN "Employee" rep ON rep."Id" = c."SupportRepId"
               WHERE i."Id" = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(common::database)?
        else {
            return Ok(None);
        };

        let invoice = rows::invoice(&header).map_err(common::database)?;

        let rep_first = rows::text(&header, "RepFirstName").map_err(common::database)?;
        let rep_last = rows::text(&header, "RepLastName").map_err(common::database)?;

        let customer = rows::int(&header, "CustomerKey")
            .map_err(common::database)?
            .map(|customer_id| {
                Ok::<_, sqlx::Error>(CustomerApiModel {
                    id: customer_id,
                    first_name: rows::text(&header, "CustomerFirstName")?,
                    last_name: rows::text(&header, "CustomerLastName")?,
                    company: rows::text(&header, "CustomerCompany")?,
                    email: rows::text(&header, "CustomerEmail")?,
                    phone: rows::text(&header, "CustomerPhone")?,
                    support_rep_id: rows::int(&header, "CustomerSupportRepId")?,
                    support_rep_name: match (&rep_first, &rep_last) {
                        (None, None) => None,
                        (first, last) => Some(format!(
                            "{} {}",
                            first.clone().unwrap_or_default(),
                            last.clone().unwrap_or_default()
                        )),
                    },
                    ..CustomerApiModel::default()
                })
            })
            .transpose()
            .map_err(common::database)?;

        let line_rows = sqlx::query(
            r#"SELECT il."Id", il."InvoiceId", il."TrackId", il."UnitPrice", il."Quantity",
                      t."Name" AS "TrackName"
               FROM "InvoiceLine" il
               LEFT JOIN "Track" t ON t."Id" = il."TrackId"
               WHERE il."InvoiceId" = ?
               ORDER BY il."Id""#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let mut invoice_lines = Vec::with_capacity(line_rows.len());
        for row in &line_rows {
            let line = rows::invoice_line(row).map_err(common::database)?;
            invoice_lines.push(InvoiceLineApiModel {
                id: line.id,
                invoice_id: line.invoice_id,
                track_id: line.track_id,
                track_name: rows::text(row, "TrackName").map_err(common::database)?,
                unit_price: line.unit_price,
                quantity: line.quantity,
                invoice: None,
                track: None,
            });
        }

        Ok(Some(InvoiceApiModel {
            id: invoice.id,
            customer_id: invoice.customer_id,
            invoice_date: invoice.invoice_date,
            billing_address: invoice.billing_address,
            billing_city: invoice.billing_city,
            billing_state: invoice.billing_state,
            billing_country: invoice.billing_country,
            billing_postal_code: invoice.billing_postal_code,
            total: invoice.total,
            customer,
            invoice_lines,
        }))
    }
}

/// sqlx implementation of [`InvoiceLineRepository`].
#[derive(Debug, Clone)]
pub struct SqliteInvoiceLineRepository {
    pool: SqlitePool,
}

impl SqliteInvoiceLineRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn lines_where(&self, clause: &str, id: i32) -> RepositoryResult<Vec<InvoiceLine>> {
        let statement =
            format!(r#"SELECT {INVOICE_LINE_COLUMNS} FROM "InvoiceLine" {clause} ORDER BY "Id""#);

        let rows = sqlx::query(&statement)
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::invoice_line)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }
}

#[async_trait]
impl Repository<InvoiceLine> for SqliteInvoiceLineRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::INVOICE_LINE, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<InvoiceLine>> {
        let rows = sqlx::query(&format!(
            r#"SELECT {INVOICE_LINE_COLUMNS} FROM "InvoiceLine" ORDER BY "Id""#
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::invoice_line)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: InvoiceLine) -> RepositoryResult<InvoiceLine> {
        let row = sqlx::query(&format!(
            r#"INSERT INTO "InvoiceLine" ("InvoiceId", "TrackId", "UnitPrice", "Quantity")
               VALUES (?, ?, ?, ?)
               RETURNING {INVOICE_LINE_COLUMNS}"#
        ))
        .bind(entity.invoice_id)
        .bind(entity.track_id)
        .bind(rows::money(entity.unit_price))
        .bind(entity.quantity)
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;

        rows::invoice_line(&row).map_err(common::database)
    }

    async fn update(&self, entity: InvoiceLine) -> RepositoryResult<bool> {
        if !self.entity_exists(entity.id).await? {
            return Ok(false);
        }

        sqlx::query(
            r#"UPDATE "InvoiceLine" SET "InvoiceId" = ?, "TrackId" = ?, "UnitPrice" = ?,
                                        "Quantity" = ?
               WHERE "Id" = ?"#,
        )
        .bind(entity.invoice_id)
        .bind(entity.track_id)
        .bind(rows::money(entity.unit_price))
        .bind(entity.quantity)
        .bind(entity.id)
        .execute(&self.pool)
        .await
        .map_err(common::database)?;

        Ok(true)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::INVOICE_LINE, id).await
    }
}

#[async_trait]
impl InvoiceLineRepository for SqliteInvoiceLineRepository {
    async fn get_by_invoice_id(&self, id: i32) -> RepositoryResult<Vec<InvoiceLine>> {
        self.lines_where(r#"WHERE "InvoiceId" = ?"#, id).await
    }

    async fn get_by_track_id(&self, id: i32) -> RepositoryResult<Vec<InvoiceLine>> {
        self.lines_where(r#"WHERE "TrackId" = ?"#, id).await
    }

    /// Returns the entity, not an API model.
    ///
    /// This is what makes `GET /api/orders/invoice-lines/{id}` the one endpoint
    /// in the application that serves an entity shape.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<InvoiceLine>> {
        let row = sqlx::query(&format!(
            r#"SELECT {INVOICE_LINE_COLUMNS} FROM "InvoiceLine" WHERE "Id" = ?"#
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(common::database)?;

        row.as_ref()
            .map(rows::invoice_line)
            .transpose()
            .map_err(common::database)
    }
}
