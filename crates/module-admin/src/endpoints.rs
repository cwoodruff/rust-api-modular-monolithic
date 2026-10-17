//! Administration's routes, ported from `Admin.Module/Endpoints`.
//!
//! One group at `/api/admin`, with the module's own rate limit and metrics.
//! Reads stack three policies — `role.admin`, `administration.read`,
//! `tenant.scoped` — and writes the same with `administration.write`.

use std::sync::Arc;

use identity_contracts::{permissions, policies};
use shared_kernel::endpoints::Endpoints;
use shared_kernel::health;

use crate::validation::{CreateGenreRequestValidator, UpdateGenreRequestValidator};
use crate::{AdministrationState, NAME, PREFIX, TAG, handlers};

/// Port of `RequireAdministrationReadAccess()`.
const READ: &[&str] = &[
    policies::ADMIN,
    permissions::ADMINISTRATION_READ,
    policies::TENANT_SCOPED,
];
/// Port of `RequireAdministrationWriteAccess()`.
const WRITE: &[&str] = &[
    policies::ADMIN,
    permissions::ADMINISTRATION_WRITE,
    policies::TENANT_SCOPED,
];

const BY_ID: &[u16] = &[200, 401, 403, 404, 429];
const LIST: &[u16] = &[200, 401, 403, 429];

pub(crate) fn map(endpoints: &mut Endpoints, state: &Arc<AdministrationState>) {
    let mut group = endpoints.map_group(PREFIX, TAG, Arc::clone(state));
    group
        .require_rate_limiting(state.rate_limiter.clone())
        .add_module_metrics(Arc::clone(&state.meter));

    health::map_health_endpoints(&mut group, NAME, Arc::new(state.database.clone()));

    // Customers
    group
        .map_get("/customers/{id:int}", handlers::get_customer_by_id)
        .with_name("AdministrationGetCustomerById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/customers/{id:int}/purchases", handlers::get_customer_purchases)
        .with_name("AdministrationGetCustomerPurchases")
        .with_description(
            "Total spent and invoice count, summed from InvoiceFinalized events. Eventually consistent: an invoice finalized moments ago may not be counted yet.",
        )
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/customers/", handlers::get_all_customers)
        .with_name("GetAllCustomers")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/customers/support-rep/{id:int}",
            handlers::get_customers_by_support_rep_id,
        )
        .with_name("GetCustomersBySupportRepId")
        .require_authorization(READ)
        .produces(LIST);

    // Employees
    group
        .map_get("/employees/{id:int}", handlers::get_employee_by_id)
        .with_name("AdministrationGetEmployeeById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/employees/", handlers::get_all_employees)
        .with_name("GetAllEmployees")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/employees/{id:int}/direct-reports",
            handlers::get_employee_direct_reports,
        )
        .with_name("GetEmployeeDirectReports")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_get(
            "/employees/{id:int}/reports-to",
            handlers::get_employee_reports_to,
        )
        .with_name("GetEmployeeReportsTo")
        .with_description(
            "The employee's manager. 404 when the employee does not exist or reports to no one.",
        )
        .require_authorization(READ)
        .produces(BY_ID);

    // Genres
    group
        .map_get("/genres/{id:int}", handlers::get_genre_by_id)
        .with_name("AdministrationGetGenreById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/genres/", handlers::get_all_genres)
        .with_name("GetAllGenres")
        .require_authorization(READ)
        .produces(LIST);
    group
        .map_post("/genres", handlers::create_genre)
        .with_name("CreateGenre")
        .require_authorization(WRITE)
        .with_validation::<crate::models::CreateGenreRequest>(Arc::new(CreateGenreRequestValidator))
        .produces(&[201, 400, 401, 403, 429]);
    group
        .map_put("/genres/{id:int}", handlers::update_genre)
        .with_name("UpdateGenre")
        .require_authorization(WRITE)
        .with_validation::<crate::models::UpdateGenreRequest>(Arc::new(UpdateGenreRequestValidator))
        .produces(&[200, 400, 401, 403, 404, 429]);
    group
        .map_delete("/genres/{id:int}", handlers::delete_genre)
        .with_name("DeleteGenre")
        .require_authorization(WRITE)
        .produces(&[204, 401, 403, 404, 429]);

    // Media types
    group
        .map_get("/media-types/{id:int}", handlers::get_media_type_by_id)
        .with_name("AdministrationGetMediaTypeById")
        .require_authorization(READ)
        .produces(BY_ID);
    group
        .map_get("/media-types/", handlers::get_all_media_types)
        .with_name("GetAllMediaTypes")
        .require_authorization(READ)
        .produces(LIST);
}
