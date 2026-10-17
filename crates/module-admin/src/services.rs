//! Administration's services, ported from `Admin.Module/Services`.
//!
//! Each reads its module's own tables through its own pool and caches reads
//! cache-aside for twenty minutes under upstream's keys and tags. A lookup that
//! finds nothing is not cached; purchases are never cached.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use shared_kernel::caching::{CacheEntryOptions, CacheFacade, CacheKeyComposer};
use shared_kernel::persistence::DbPool;
use shared_kernel::{ApiError, DataResult};
use sqlx::FromRow;

use crate::models::{
    CustomerApiModel, CustomerPurchasesApiModel, EmployeeApiModel, GenreApiModel, MediaTypeApiModel,
};
use crate::validation::validate_genre;

/// The cache module segment.
const MODULE: &str = "administration";
/// The schema version every key carries.
const VERSION: &str = "v1";

const CUSTOMER_TAGS: [&str; 2] = ["administration:customer", "administration:customer:by-id"];
const EMPLOYEE_TAGS: [&str; 2] = ["administration:employee", "administration:employee:by-id"];
const GENRE_TAGS: [&str; 2] = ["administration:genre", "administration:genre:by-id"];
const MEDIA_TYPE_TAGS: [&str; 2] = ["administration:mediatype", "administration:mediatype:by-id"];

/// What every Administration service holds.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    pub pool: DbPool,
    pub cache: Arc<CacheFacade>,
    pub keys: Arc<CacheKeyComposer>,
}

impl Store {
    async fn cached<T, F, Fut>(
        &self,
        entity: &str,
        discriminator: &str,
        tags: [&str; 2],
        load: F,
    ) -> DataResult<Option<T>>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = DataResult<Option<T>>> + Send,
    {
        let key = self.keys.compose(MODULE, entity, VERSION, discriminator);
        self.cache
            .try_get_or_add(&key, load, Some(CacheEntryOptions::for_service(tags)))
            .await
    }

    async fn cached_list<T, F, Fut>(
        &self,
        entity: &str,
        discriminator: &str,
        tags: [&str; 2],
        load: F,
    ) -> DataResult<Vec<T>>
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = DataResult<Vec<T>>> + Send,
    {
        Ok(self
            .cached(entity, discriminator, tags, || async {
                load().await.map(Some)
            })
            .await?
            .unwrap_or_default())
    }

    /// Drops the entity's collection entries and one by-id entry.
    ///
    /// Upstream calls `RemoveByTagAsync(tags[0])`, a no-op there; here it
    /// really drops every entry carrying the tag (F1).
    async fn invalidate(&self, entity: &str, tags: [&str; 2], id: i32) {
        self.cache.remove_by_tag(tags[0]).await;
        self.cache
            .remove(
                &self
                    .keys
                    .compose(MODULE, entity, VERSION, &format!("by-id:{id}")),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

const CUSTOMER_COLUMNS: &str = r#""Id", "FirstName", "LastName", "Company", "Address", "City", "State", "Country",
       "PostalCode", "Phone", "Fax", "Email""#;

#[derive(FromRow)]
struct CustomerRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "FirstName")]
    first_name: Option<String>,
    #[sqlx(rename = "LastName")]
    last_name: Option<String>,
    #[sqlx(rename = "Company")]
    company: Option<String>,
    #[sqlx(rename = "Address")]
    address: Option<String>,
    #[sqlx(rename = "City")]
    city: Option<String>,
    #[sqlx(rename = "State")]
    state: Option<String>,
    #[sqlx(rename = "Country")]
    country: Option<String>,
    #[sqlx(rename = "PostalCode")]
    postal_code: Option<String>,
    #[sqlx(rename = "Phone")]
    phone: Option<String>,
    #[sqlx(rename = "Fax")]
    fax: Option<String>,
    #[sqlx(rename = "Email")]
    email: Option<String>,
}

impl From<CustomerRow> for CustomerApiModel {
    /// Upstream's `ToApiModel()`: `Id` to `Email`, and not `SupportRepId`, so
    /// the collection routes always answer `SupportRepId: null`.
    fn from(row: CustomerRow) -> Self {
        Self {
            id: row.id,
            first_name: row.first_name,
            last_name: row.last_name,
            company: row.company,
            address: row.address,
            city: row.city,
            state: row.state,
            country: row.country,
            postal_code: row.postal_code,
            phone: row.phone,
            fax: row.fax,
            email: row.email,
            support_rep_id: None,
            support_rep_name: None,
            support_rep: None,
        }
    }
}

#[derive(FromRow)]
struct CustomerByIdRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "FirstName")]
    first_name: Option<String>,
    #[sqlx(rename = "LastName")]
    last_name: Option<String>,
    #[sqlx(rename = "SupportRepId")]
    support_rep_id: Option<i32>,
    #[sqlx(rename = "SupportRepName")]
    support_rep_name: Option<String>,
    #[sqlx(rename = "RepId")]
    rep_id: Option<i32>,
    #[sqlx(rename = "RepFirstName")]
    rep_first_name: Option<String>,
    #[sqlx(rename = "RepLastName")]
    rep_last_name: Option<String>,
    #[sqlx(rename = "RepTitle")]
    rep_title: Option<String>,
}

const EMPLOYEE_COLUMNS: &str = r#"e."Id", e."LastName", e."FirstName", e."Title", e."ReportsTo", e."BirthDate",
       e."HireDate", e."Address", e."City", e."State", e."Country", e."PostalCode", e."Phone", e."Fax", e."Email""#;

#[derive(FromRow)]
struct EmployeeRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "LastName")]
    last_name: Option<String>,
    #[sqlx(rename = "FirstName")]
    first_name: Option<String>,
    #[sqlx(rename = "Title")]
    title: Option<String>,
    #[sqlx(rename = "ReportsTo")]
    reports_to: Option<i32>,
    #[sqlx(rename = "BirthDate")]
    birth_date: Option<DateTime<Utc>>,
    #[sqlx(rename = "HireDate")]
    hire_date: Option<DateTime<Utc>>,
    #[sqlx(rename = "Address")]
    address: Option<String>,
    #[sqlx(rename = "City")]
    city: Option<String>,
    #[sqlx(rename = "State")]
    state: Option<String>,
    #[sqlx(rename = "Country")]
    country: Option<String>,
    #[sqlx(rename = "PostalCode")]
    postal_code: Option<String>,
    #[sqlx(rename = "Phone")]
    phone: Option<String>,
    #[sqlx(rename = "Fax")]
    fax: Option<String>,
    #[sqlx(rename = "Email")]
    email: Option<String>,
    #[sqlx(rename = "ReportsToNavigation", default)]
    reports_to_navigation: Option<String>,
}

impl From<EmployeeRow> for EmployeeApiModel {
    fn from(row: EmployeeRow) -> Self {
        Self {
            id: row.id,
            last_name: row.last_name,
            first_name: row.first_name,
            title: row.title,
            reports_to: row.reports_to,
            birth_date: row.birth_date,
            hire_date: row.hire_date,
            address: row.address,
            city: row.city,
            state: row.state,
            country: row.country,
            postal_code: row.postal_code,
            phone: row.phone,
            fax: row.fax,
            email: row.email,
            customers: Vec::new(),
            inverse_reports_to_navigation: Vec::new(),
            reports_to_navigation: row.reports_to_navigation,
        }
    }
}

#[derive(FromRow)]
struct NamedRow {
    #[sqlx(rename = "Id")]
    id: i32,
    #[sqlx(rename = "Name")]
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// Customers
// ---------------------------------------------------------------------------

/// Port of `ICustomerService`.
#[async_trait]
pub(crate) trait CustomerService: Send + Sync {
    async fn get_customer_by_id(&self, id: i32) -> DataResult<Option<CustomerApiModel>>;
    async fn get_all_customers(&self) -> DataResult<Vec<CustomerApiModel>>;
    async fn get_customers_by_support_rep_id(&self, id: i32) -> DataResult<Vec<CustomerApiModel>>;
    async fn get_customer_purchases(
        &self,
        id: i32,
    ) -> DataResult<Option<CustomerPurchasesApiModel>>;
}

/// Port of `CustomerService`.
#[derive(Debug, Clone)]
pub(crate) struct Customers(pub Store);

#[async_trait]
impl CustomerService for Customers {
    /// The by-id projection fills `Id`, the names and the support rep only —
    /// the nine other columns are never selected upstream, so they answer null
    /// (P17, P19). The rep's name is SQL concatenation: null if either half is.
    async fn get_customer_by_id(&self, id: i32) -> DataResult<Option<CustomerApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached(
                "customer",
                &format!("by-id:{id}"),
                CUSTOMER_TAGS,
                || async {
                    let row = sqlx::query_as::<_, CustomerByIdRow>(
                        r#"SELECT c."Id", c."FirstName", c."LastName", c."SupportRepId",
                              rep."FirstName" || ' ' || rep."LastName" AS "SupportRepName",
                              rep."Id" AS "RepId", rep."FirstName" AS "RepFirstName",
                              rep."LastName" AS "RepLastName", rep."Title" AS "RepTitle"
                         FROM administration."Customer" c
                         LEFT JOIN administration."Employee" rep ON rep."Id" = c."SupportRepId"
                        WHERE c."Id" = $1"#,
                    )
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;

                    Ok(row.map(|row| CustomerApiModel {
                        id: row.id,
                        first_name: row.first_name,
                        last_name: row.last_name,
                        support_rep_id: row.support_rep_id,
                        support_rep_name: row.support_rep_name,
                        support_rep: row.rep_id.map(|rep_id| EmployeeApiModel {
                            id: rep_id,
                            first_name: row.rep_first_name,
                            last_name: row.rep_last_name,
                            title: row.rep_title,
                            ..EmployeeApiModel::default()
                        }),
                        ..CustomerApiModel::default()
                    }))
                },
            )
            .await
    }

    async fn get_all_customers(&self) -> DataResult<Vec<CustomerApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("customer", "all", CUSTOMER_TAGS, || async {
                let rows = sqlx::query_as::<_, CustomerRow>(&format!(
                    r#"SELECT {CUSTOMER_COLUMNS} FROM administration."Customer" ORDER BY "Id""#
                ))
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            })
            .await
    }

    async fn get_customers_by_support_rep_id(&self, id: i32) -> DataResult<Vec<CustomerApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("customer", &format!("by-supportrep:{id}"), CUSTOMER_TAGS, || async {
                let rows = sqlx::query_as::<_, CustomerRow>(&format!(
                    r#"SELECT {CUSTOMER_COLUMNS} FROM administration."Customer" WHERE "SupportRepId" = $1 ORDER BY "Id""#
                ))
                .bind(id)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            })
            .await
    }

    async fn get_customer_purchases(
        &self,
        id: i32,
    ) -> DataResult<Option<CustomerPurchasesApiModel>> {
        let pool = &self.0.pool;
        let exists: bool = sqlx::query_scalar(
            r#"SELECT EXISTS (SELECT 1 FROM administration."Customer" WHERE "Id" = $1)"#,
        )
        .bind(id)
        .fetch_one(pool)
        .await?;
        if !exists {
            return Ok(None);
        }

        let summary: Option<(Decimal, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"SELECT "TotalSpent", "InvoiceCount", "LastPurchaseAt"
                 FROM administration."CustomerPurchaseSummary" WHERE "CustomerId" = $1"#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;

        let (total_spent, invoice_count, last_purchase_at) =
            summary.unwrap_or((Decimal::ZERO, 0, None));
        Ok(Some(CustomerPurchasesApiModel {
            customer_id: id,
            total_spent,
            invoice_count,
            last_purchase_at,
        }))
    }
}

// ---------------------------------------------------------------------------
// Employees
// ---------------------------------------------------------------------------

/// Port of `IEmployeeService`.
#[async_trait]
pub(crate) trait EmployeeService: Send + Sync {
    async fn get_employee_by_id(&self, id: i32) -> DataResult<Option<EmployeeApiModel>>;
    async fn get_all_employees(&self) -> DataResult<Vec<EmployeeApiModel>>;
    async fn get_direct_reports(&self, id: i32) -> DataResult<Vec<EmployeeApiModel>>;
    async fn get_reports_to(&self, id: i32) -> DataResult<Option<EmployeeApiModel>>;
}

/// Port of `EmployeeService`.
#[derive(Debug, Clone)]
pub(crate) struct Employees(pub Store);

#[async_trait]
impl EmployeeService for Employees {
    async fn get_employee_by_id(&self, id: i32) -> DataResult<Option<EmployeeApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached("employee", &format!("by-id:{id}"), EMPLOYEE_TAGS, || async {
                let row = sqlx::query_as::<_, EmployeeRow>(&format!(
                    r#"SELECT {EMPLOYEE_COLUMNS}, manager."FirstName" || ' ' || manager."LastName" AS "ReportsToNavigation"
                         FROM administration."Employee" e
                         LEFT JOIN administration."Employee" manager ON manager."Id" = e."ReportsTo"
                        WHERE e."Id" = $1"#
                ))
                .bind(id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(Into::into))
            })
            .await
    }

    async fn get_all_employees(&self) -> DataResult<Vec<EmployeeApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("employee", "all", EMPLOYEE_TAGS, || async {
                let rows = sqlx::query_as::<_, EmployeeRow>(&format!(
                    r#"SELECT {EMPLOYEE_COLUMNS} FROM administration."Employee" e ORDER BY e."Id""#
                ))
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            })
            .await
    }

    async fn get_direct_reports(&self, id: i32) -> DataResult<Vec<EmployeeApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("employee", &format!("direct-reports:{id}"), EMPLOYEE_TAGS, || async {
                let rows = sqlx::query_as::<_, EmployeeRow>(&format!(
                    r#"SELECT {EMPLOYEE_COLUMNS} FROM administration."Employee" e WHERE e."ReportsTo" = $1 ORDER BY e."Id""#
                ))
                .bind(id)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            })
            .await
    }

    /// The employee's manager (665e49c). Nothing when the employee does not
    /// exist or reports to no one.
    async fn get_reports_to(&self, id: i32) -> DataResult<Option<EmployeeApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached(
                "employee",
                &format!("reports-to:{id}"),
                EMPLOYEE_TAGS,
                || async {
                    let row = sqlx::query_as::<_, EmployeeRow>(&format!(
                        r#"SELECT {EMPLOYEE_COLUMNS} FROM administration."Employee" e
                        WHERE EXISTS (SELECT 1 FROM administration."Employee" report
                                       WHERE report."Id" = $1 AND report."ReportsTo" = e."Id")"#
                    ))
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
                    Ok(row.map(Into::into))
                },
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Genres
// ---------------------------------------------------------------------------

/// Port of `IGenreService`.
#[async_trait]
pub(crate) trait GenreService: Send + Sync {
    async fn get_genre_by_id(&self, id: i32) -> DataResult<Option<GenreApiModel>>;
    async fn get_all_genres(&self) -> DataResult<Vec<GenreApiModel>>;
    async fn create_genre(&self, name: Option<String>) -> Result<GenreApiModel, ApiError>;
    async fn update_genre(&self, id: i32, name: Option<String>) -> Result<bool, ApiError>;
    async fn delete_genre(&self, id: i32) -> Result<bool, ApiError>;
}

/// Port of `GenreService`.
#[derive(Debug, Clone)]
pub(crate) struct Genres(pub Store);

#[async_trait]
impl GenreService for Genres {
    async fn get_genre_by_id(&self, id: i32) -> DataResult<Option<GenreApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached("genre", &format!("by-id:{id}"), GENRE_TAGS, || async {
                let row = sqlx::query_as::<_, NamedRow>(
                    r#"SELECT "Id", "Name" FROM administration."Genre" WHERE "Id" = $1"#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|row| GenreApiModel {
                    id: row.id,
                    name: row.name,
                }))
            })
            .await
    }

    async fn get_all_genres(&self) -> DataResult<Vec<GenreApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("genre", "all", GENRE_TAGS, || async {
                let rows = sqlx::query_as::<_, NamedRow>(
                    r#"SELECT "Id", "Name" FROM administration."Genre" ORDER BY "Id""#,
                )
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|row| GenreApiModel {
                        id: row.id,
                        name: row.name,
                    })
                    .collect())
            })
            .await
    }

    async fn create_genre(&self, name: Option<String>) -> Result<GenreApiModel, ApiError> {
        validate_genre(name.as_deref()).map_err(ApiError::Validation)?;

        let row = sqlx::query_as::<_, NamedRow>(
            r#"INSERT INTO administration."Genre" ("Name") VALUES ($1) RETURNING "Id", "Name""#,
        )
        .bind(&name)
        .fetch_one(&self.0.pool)
        .await
        .map_err(shared_kernel::DataError::from)?;

        self.0.invalidate("genre", GENRE_TAGS, row.id).await;
        Ok(GenreApiModel {
            id: row.id,
            name: row.name,
        })
    }

    /// One statement reporting what it changed, rather than a check then a
    /// write (F11).
    async fn update_genre(&self, id: i32, name: Option<String>) -> Result<bool, ApiError> {
        validate_genre(name.as_deref()).map_err(ApiError::Validation)?;

        let updated =
            sqlx::query(r#"UPDATE administration."Genre" SET "Name" = $1 WHERE "Id" = $2"#)
                .bind(&name)
                .bind(id)
                .execute(&self.0.pool)
                .await
                .map_err(shared_kernel::DataError::from)?
                .rows_affected()
                > 0;

        if updated {
            self.0.invalidate("genre", GENRE_TAGS, id).await;
        }
        Ok(updated)
    }

    /// A genre still referenced by tracks deletes: no foreign key crosses into
    /// Catalog any more (ADR-0006), and the orphan check reports what is left.
    async fn delete_genre(&self, id: i32) -> Result<bool, ApiError> {
        let deleted = sqlx::query(r#"DELETE FROM administration."Genre" WHERE "Id" = $1"#)
            .bind(id)
            .execute(&self.0.pool)
            .await
            .map_err(shared_kernel::DataError::from)?
            .rows_affected()
            > 0;

        if deleted {
            self.0.invalidate("genre", GENRE_TAGS, id).await;
        }
        Ok(deleted)
    }
}

// ---------------------------------------------------------------------------
// Media types
// ---------------------------------------------------------------------------

/// Port of `IMediaTypeService`.
#[async_trait]
pub(crate) trait MediaTypeService: Send + Sync {
    async fn get_media_type_by_id(&self, id: i32) -> DataResult<Option<MediaTypeApiModel>>;
    async fn get_all_media_types(&self) -> DataResult<Vec<MediaTypeApiModel>>;
}

/// Port of `MediaTypeService`.
#[derive(Debug, Clone)]
pub(crate) struct MediaTypes(pub Store);

#[async_trait]
impl MediaTypeService for MediaTypes {
    async fn get_media_type_by_id(&self, id: i32) -> DataResult<Option<MediaTypeApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached(
                "mediatype",
                &format!("by-id:{id}"),
                MEDIA_TYPE_TAGS,
                || async {
                    let row = sqlx::query_as::<_, NamedRow>(
                        r#"SELECT "Id", "Name" FROM administration."MediaType" WHERE "Id" = $1"#,
                    )
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
                    Ok(row.map(|row| MediaTypeApiModel {
                        id: row.id,
                        name: row.name,
                    }))
                },
            )
            .await
    }

    async fn get_all_media_types(&self) -> DataResult<Vec<MediaTypeApiModel>> {
        let pool = &self.0.pool;
        self.0
            .cached_list("mediatype", "all", MEDIA_TYPE_TAGS, || async {
                let rows = sqlx::query_as::<_, NamedRow>(
                    r#"SELECT "Id", "Name" FROM administration."MediaType" ORDER BY "Id""#,
                )
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|row| MediaTypeApiModel {
                        id: row.id,
                        name: row.name,
                    })
                    .collect())
            })
            .await
    }
}
