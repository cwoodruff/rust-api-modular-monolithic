//! Customer, employee, genre, and media type repositories.

use async_trait::async_trait;
use shared_persistence::api_models::{CustomerApiModel, EmployeeApiModel, InvoiceApiModel};
use shared_persistence::entities::{Customer, Employee, Genre, MediaType};
use shared_persistence::repositories::{
    CustomerRepository, EmployeeRepository, GenreRepository, MediaTypeRepository, Repository,
    RepositoryResult,
};
use sqlx::SqlitePool;

use super::common::{self, tables};
use crate::rows;

/// Every scalar column on `Customer`.
const CUSTOMER_COLUMNS: &str = r#""Id", "FirstName", "LastName", "Company", "Address", "City",
    "State", "Country", "PostalCode", "Phone", "Fax", "Email", "SupportRepId""#;

/// Every scalar column on `Employee`.
const EMPLOYEE_COLUMNS: &str = r#""Id", "LastName", "FirstName", "Title", "ReportsTo", "BirthDate",
    "HireDate", "Address", "City", "State", "Country", "PostalCode", "Phone", "Fax", "Email""#;

/// sqlx implementation of [`CustomerRepository`].
#[derive(Debug, Clone)]
pub struct SqliteCustomerRepository {
    pool: SqlitePool,
}

impl SqliteCustomerRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<Customer> for SqliteCustomerRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::CUSTOMER, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Customer>> {
        let rows = sqlx::query(&format!(
            r#"SELECT {CUSTOMER_COLUMNS} FROM "Customer" ORDER BY "Id""#
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::customer)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Customer) -> RepositoryResult<Customer> {
        let row = sqlx::query(&format!(
            r#"INSERT INTO "Customer" ("FirstName", "LastName", "Company", "Address", "City",
                                       "State", "Country", "PostalCode", "Phone", "Fax", "Email",
                                       "SupportRepId")
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               RETURNING {CUSTOMER_COLUMNS}"#
        ))
        .bind(&entity.first_name)
        .bind(&entity.last_name)
        .bind(&entity.company)
        .bind(&entity.address)
        .bind(&entity.city)
        .bind(&entity.state)
        .bind(&entity.country)
        .bind(&entity.postal_code)
        .bind(&entity.phone)
        .bind(&entity.fax)
        .bind(&entity.email)
        .bind(entity.support_rep_id)
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;

        rows::customer(&row).map_err(common::database)
    }

    async fn update(&self, entity: Customer) -> RepositoryResult<bool> {
        if !self.entity_exists(entity.id).await? {
            return Ok(false);
        }

        sqlx::query(
            r#"UPDATE "Customer" SET "FirstName" = ?, "LastName" = ?, "Company" = ?, "Address" = ?,
                                     "City" = ?, "State" = ?, "Country" = ?, "PostalCode" = ?,
                                     "Phone" = ?, "Fax" = ?, "Email" = ?, "SupportRepId" = ?
               WHERE "Id" = ?"#,
        )
        .bind(&entity.first_name)
        .bind(&entity.last_name)
        .bind(&entity.company)
        .bind(&entity.address)
        .bind(&entity.city)
        .bind(&entity.state)
        .bind(&entity.country)
        .bind(&entity.postal_code)
        .bind(&entity.phone)
        .bind(&entity.fax)
        .bind(&entity.email)
        .bind(entity.support_rep_id)
        .bind(entity.id)
        .execute(&self.pool)
        .await
        .map_err(common::database)?;

        Ok(true)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::CUSTOMER, id).await
    }
}

#[async_trait]
impl CustomerRepository for SqliteCustomerRepository {
    async fn get_by_support_rep_id(&self, id: i32) -> RepositoryResult<Vec<Customer>> {
        let rows = sqlx::query(&format!(
            r#"SELECT {CUSTOMER_COLUMNS} FROM "Customer" WHERE "SupportRepId" = ? ORDER BY "Id""#
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::customer)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    /// The customer with their support representative and invoices.
    ///
    /// # A gap kept on purpose
    ///
    /// The C# projection lists `Id`, `FirstName`, `LastName`, then the literal
    /// comment `// ... other fields ...`, and resumes at `SupportRepId`. The
    /// nine columns that comment stands in for — company, address, city, state,
    /// country, postal code, phone, fax, email — are never projected, so
    /// `GET /api/admin/customers/{id}` answers with all nine as `null` even
    /// though the row has values for them.
    ///
    /// That is wire-visible, so this reproduces it rather than filling the gap
    /// in. Filling it in is a one-line change here whenever the original fixes
    /// it. Note the *collection* endpoint does return those fields, because it
    /// goes through the entity conversion instead — so the same customer looks
    /// different depending on which endpoint served it.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<CustomerApiModel>> {
        let Some(header) = sqlx::query(
            r#"SELECT c."Id", c."FirstName", c."LastName", c."SupportRepId",
                      rep."Id" AS "RepId", rep."FirstName" AS "RepFirstName",
                      rep."LastName" AS "RepLastName", rep."Title" AS "RepTitle"
               FROM "Customer" c
               LEFT JOIN "Employee" rep ON rep."Id" = c."SupportRepId"
               WHERE c."Id" = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(common::database)?
        else {
            return Ok(None);
        };

        let support_rep_id = rows::int(&header, "SupportRepId").map_err(common::database)?;
        let rep_first = rows::text(&header, "RepFirstName").map_err(common::database)?;
        let rep_last = rows::text(&header, "RepLastName").map_err(common::database)?;

        let support_rep = rows::int(&header, "RepId")
            .map_err(common::database)?
            .map(|rep_id| EmployeeApiModel {
                id: rep_id,
                first_name: rep_first.clone(),
                last_name: rep_last.clone(),
                title: rows::text(&header, "RepTitle").unwrap_or_default(),
                ..EmployeeApiModel::default()
            });

        let invoice_rows = sqlx::query(
            r#"SELECT "Id", "CustomerId", "InvoiceDate", "BillingAddress", "BillingCity",
                      "BillingState", "BillingCountry", "BillingPostalCode", "Total"
               FROM "Invoice" WHERE "CustomerId" = ? ORDER BY "Id""#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        let mut invoices = Vec::with_capacity(invoice_rows.len());
        for row in &invoice_rows {
            let invoice = rows::invoice(row).map_err(common::database)?;
            invoices.push(InvoiceApiModel {
                id: invoice.id,
                customer_id: invoice.customer_id,
                invoice_date: invoice.invoice_date,
                billing_address: invoice.billing_address,
                billing_city: invoice.billing_city,
                billing_state: invoice.billing_state,
                billing_country: invoice.billing_country,
                billing_postal_code: invoice.billing_postal_code,
                total: invoice.total,
                // No back-reference, and lines are not projected here.
                customer: None,
                invoice_lines: Vec::new(),
            });
        }

        Ok(Some(CustomerApiModel {
            id: rows::id(&header, "Id").map_err(common::database)?,
            first_name: rows::text(&header, "FirstName").map_err(common::database)?,
            last_name: rows::text(&header, "LastName").map_err(common::database)?,
            // The nine unprojected columns. See the note above.
            company: None,
            address: None,
            city: None,
            state: None,
            country: None,
            postal_code: None,
            phone: None,
            fax: None,
            email: None,
            support_rep_id,
            support_rep_name: match (rep_first, rep_last) {
                (None, None) => None,
                (first, last) => Some(format!(
                    "{} {}",
                    first.unwrap_or_default(),
                    last.unwrap_or_default()
                )),
            },
            invoices,
            support_rep,
        }))
    }
}

/// sqlx implementation of [`EmployeeRepository`].
#[derive(Debug, Clone)]
pub struct SqliteEmployeeRepository {
    pool: SqlitePool,
}

impl SqliteEmployeeRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn employees_where(&self, clause: &str, id: i32) -> RepositoryResult<Vec<Employee>> {
        let statement =
            format!(r#"SELECT {EMPLOYEE_COLUMNS} FROM "Employee" {clause} ORDER BY "Id""#);

        let rows = sqlx::query(&statement)
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::employee)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }
}

#[async_trait]
impl Repository<Employee> for SqliteEmployeeRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::EMPLOYEE, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Employee>> {
        let rows = sqlx::query(&format!(
            r#"SELECT {EMPLOYEE_COLUMNS} FROM "Employee" ORDER BY "Id""#
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(common::database)?;

        rows.iter()
            .map(rows::employee)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Employee) -> RepositoryResult<Employee> {
        let row = sqlx::query(&format!(
            r#"INSERT INTO "Employee" ("LastName", "FirstName", "Title", "ReportsTo", "BirthDate",
                                       "HireDate", "Address", "City", "State", "Country",
                                       "PostalCode", "Phone", "Fax", "Email")
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               RETURNING {EMPLOYEE_COLUMNS}"#
        ))
        .bind(&entity.last_name)
        .bind(&entity.first_name)
        .bind(&entity.title)
        .bind(entity.reports_to)
        .bind(rows::timestamp_text(entity.birth_date))
        .bind(rows::timestamp_text(entity.hire_date))
        .bind(&entity.address)
        .bind(&entity.city)
        .bind(&entity.state)
        .bind(&entity.country)
        .bind(&entity.postal_code)
        .bind(&entity.phone)
        .bind(&entity.fax)
        .bind(&entity.email)
        .fetch_one(&self.pool)
        .await
        .map_err(common::database)?;

        rows::employee(&row).map_err(common::database)
    }

    async fn update(&self, entity: Employee) -> RepositoryResult<bool> {
        if !self.entity_exists(entity.id).await? {
            return Ok(false);
        }

        sqlx::query(
            r#"UPDATE "Employee" SET "LastName" = ?, "FirstName" = ?, "Title" = ?, "ReportsTo" = ?,
                                     "BirthDate" = ?, "HireDate" = ?, "Address" = ?, "City" = ?,
                                     "State" = ?, "Country" = ?, "PostalCode" = ?, "Phone" = ?,
                                     "Fax" = ?, "Email" = ?
               WHERE "Id" = ?"#,
        )
        .bind(&entity.last_name)
        .bind(&entity.first_name)
        .bind(&entity.title)
        .bind(entity.reports_to)
        .bind(rows::timestamp_text(entity.birth_date))
        .bind(rows::timestamp_text(entity.hire_date))
        .bind(&entity.address)
        .bind(&entity.city)
        .bind(&entity.state)
        .bind(&entity.country)
        .bind(&entity.postal_code)
        .bind(&entity.phone)
        .bind(&entity.fax)
        .bind(&entity.email)
        .bind(entity.id)
        .execute(&self.pool)
        .await
        .map_err(common::database)?;

        Ok(true)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::EMPLOYEE, id).await
    }
}

#[async_trait]
impl EmployeeRepository for SqliteEmployeeRepository {
    /// Looks the employee up by their own key.
    ///
    /// The C# method is named `GetReportsTo` but calls `FindAsync(id)`, so it
    /// returns the employee with that key rather than that employee's manager.
    /// The endpoint reads as "the manager" only because the *service* is what
    /// decides which key to pass. Reproduced as written.
    async fn get_reports_to(&self, id: i32) -> RepositoryResult<Option<Employee>> {
        let row = sqlx::query(&format!(
            r#"SELECT {EMPLOYEE_COLUMNS} FROM "Employee" WHERE "Id" = ?"#
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(common::database)?;

        row.as_ref()
            .map(rows::employee)
            .transpose()
            .map_err(common::database)
    }

    async fn get_direct_reports(&self, id: i32) -> RepositoryResult<Vec<Employee>> {
        self.employees_where(r#"WHERE "ReportsTo" = ?"#, id).await
    }

    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<EmployeeApiModel>> {
        let Some(row) = sqlx::query(
            r#"SELECT e."Id", e."LastName", e."FirstName", e."Title", e."ReportsTo", e."BirthDate",
                      e."HireDate", e."Address", e."City", e."State", e."Country", e."PostalCode",
                      e."Phone", e."Fax", e."Email",
                      manager."FirstName" AS "ManagerFirstName",
                      manager."LastName" AS "ManagerLastName"
               FROM "Employee" e
               LEFT JOIN "Employee" manager ON manager."Id" = e."ReportsTo"
               WHERE e."Id" = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(common::database)?
        else {
            return Ok(None);
        };

        let employee = rows::employee(&row).map_err(common::database)?;
        let manager_first = rows::text(&row, "ManagerFirstName").map_err(common::database)?;
        let manager_last = rows::text(&row, "ManagerLastName").map_err(common::database)?;

        Ok(Some(EmployeeApiModel {
            id: employee.id,
            last_name: employee.last_name,
            first_name: employee.first_name,
            title: employee.title,
            reports_to: employee.reports_to,
            birth_date: employee.birth_date,
            hire_date: employee.hire_date,
            address: employee.address,
            city: employee.city,
            state: employee.state,
            country: employee.country,
            postal_code: employee.postal_code,
            phone: employee.phone,
            fax: employee.fax,
            email: employee.email,
            // Left empty to avoid the cycles the original avoids by hand.
            customers: Vec::new(),
            inverse_reports_to_navigation: Vec::new(),
            // The manager flattened to a name, not an object.
            reports_to_navigation: match (manager_first, manager_last) {
                (None, None) => None,
                (first, last) => Some(format!(
                    "{} {}",
                    first.unwrap_or_default(),
                    last.unwrap_or_default()
                )),
            },
        }))
    }
}

/// sqlx implementation of [`GenreRepository`].
///
/// The only repository whose write path any endpoint reaches.
#[derive(Debug, Clone)]
pub struct SqliteGenreRepository {
    pool: SqlitePool,
}

impl SqliteGenreRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<Genre> for SqliteGenreRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::GENRE, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<Genre>> {
        let rows = sqlx::query(r#"SELECT "Id", "Name" FROM "Genre" ORDER BY "Id""#)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::genre)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: Genre) -> RepositoryResult<Genre> {
        let row = sqlx::query(r#"INSERT INTO "Genre" ("Name") VALUES (?) RETURNING "Id", "Name""#)
            .bind(&entity.name)
            .fetch_one(&self.pool)
            .await
            .map_err(common::database)?;

        rows::genre(&row).map_err(common::database)
    }

    async fn update(&self, entity: Genre) -> RepositoryResult<bool> {
        if !self.entity_exists(entity.id).await? {
            return Ok(false);
        }

        sqlx::query(r#"UPDATE "Genre" SET "Name" = ? WHERE "Id" = ?"#)
            .bind(&entity.name)
            .bind(entity.id)
            .execute(&self.pool)
            .await
            .map_err(common::database)?;

        Ok(true)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::GENRE, id).await
    }
}

#[async_trait]
impl GenreRepository for SqliteGenreRepository {
    /// Returns the entity, not an API model — the service converts.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<Genre>> {
        let row = sqlx::query(r#"SELECT "Id", "Name" FROM "Genre" WHERE "Id" = ?"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(common::database)?;

        row.as_ref()
            .map(rows::genre)
            .transpose()
            .map_err(common::database)
    }
}

/// sqlx implementation of [`MediaTypeRepository`].
#[derive(Debug, Clone)]
pub struct SqliteMediaTypeRepository {
    pool: SqlitePool,
}

impl SqliteMediaTypeRepository {
    /// Binds the repository to a pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository<MediaType> for SqliteMediaTypeRepository {
    async fn entity_exists(&self, id: i32) -> RepositoryResult<bool> {
        common::exists(&self.pool, tables::MEDIA_TYPE, id).await
    }

    async fn get_all(&self) -> RepositoryResult<Vec<MediaType>> {
        let rows = sqlx::query(r#"SELECT "Id", "Name" FROM "MediaType" ORDER BY "Id""#)
            .fetch_all(&self.pool)
            .await
            .map_err(common::database)?;

        rows.iter()
            .map(rows::media_type)
            .collect::<Result<Vec<_>, _>>()
            .map_err(common::database)
    }

    async fn add(&self, entity: MediaType) -> RepositoryResult<MediaType> {
        let row =
            sqlx::query(r#"INSERT INTO "MediaType" ("Name") VALUES (?) RETURNING "Id", "Name""#)
                .bind(&entity.name)
                .fetch_one(&self.pool)
                .await
                .map_err(common::database)?;

        rows::media_type(&row).map_err(common::database)
    }

    async fn update(&self, entity: MediaType) -> RepositoryResult<bool> {
        if !self.entity_exists(entity.id).await? {
            return Ok(false);
        }

        sqlx::query(r#"UPDATE "MediaType" SET "Name" = ? WHERE "Id" = ?"#)
            .bind(&entity.name)
            .bind(entity.id)
            .execute(&self.pool)
            .await
            .map_err(common::database)?;

        Ok(true)
    }

    async fn delete(&self, id: i32) -> RepositoryResult<bool> {
        common::delete_by_id(&self.pool, tables::MEDIA_TYPE, id).await
    }
}

#[async_trait]
impl MediaTypeRepository for SqliteMediaTypeRepository {
    /// Returns the entity, not an API model — the service converts.
    async fn get_by_id(&self, id: i32) -> RepositoryResult<Option<MediaType>> {
        let row = sqlx::query(r#"SELECT "Id", "Name" FROM "MediaType" WHERE "Id" = ?"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(common::database)?;

        row.as_ref()
            .map(rows::media_type)
            .transpose()
            .map_err(common::database)
    }
}
