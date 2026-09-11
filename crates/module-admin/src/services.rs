//! The service layer, ported from `Admin.Modules.Services`.
//!
//! The reads follow the same cache-aside shape as every other module. The
//! Genre writes are the only ones any endpoint reaches, and they are where
//! validation and cache invalidation actually run.

use axum::response::{IntoResponse, Response};
use shared_kernel::ProblemDetails;
use shared_kernel::caching::CacheEntryOptions;
use shared_kernel::caching::tags::administration;
use shared_persistence::AppState;
use shared_persistence::api_models::{
    CustomerApiModel, EmployeeApiModel, GenreApiModel, MediaTypeApiModel,
};
use shared_persistence::convert::{Convert, convert_all};
use shared_persistence::entities::Genre;
use shared_persistence::repositories::{RepositoryError, RepositoryResult};
use shared_persistence::validation::{Validate, ValidationFailure, problem_details};

/// The cache module segment these services compose keys under.
const MODULE: &str = "administration";

/// The schema version every key carries.
const VERSION: &str = "v1";

const CUSTOMER_TAGS: [&str; 2] = [administration::CUSTOMER, "administration:customer:by-id"];
const EMPLOYEE_TAGS: [&str; 2] = [administration::EMPLOYEE, "administration:employee:by-id"];
const GENRE_TAGS: [&str; 2] = [administration::GENRE, "administration:genre:by-id"];
const MEDIA_TYPE_TAGS: [&str; 2] = [administration::MEDIA_TYPE, "administration:mediatype:by-id"];

fn entry_options(tags: [&str; 2]) -> CacheEntryOptions {
    CacheEntryOptions::for_service(tags)
}

/// A write that did not happen.
#[derive(Debug)]
pub(crate) enum WriteFailure {
    /// The model broke a validation rule.
    ///
    /// The C# service throws a `ValidationException` and the host's exception
    /// handler turns it into a 400 carrying the per-field messages.
    Validation(Vec<ValidationFailure>),

    /// The database refused.
    Repository(RepositoryError),
}

impl From<RepositoryError> for WriteFailure {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl IntoResponse for WriteFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Validation(failures) => {
                problem_details(failures, shared_kernel::errors::new_trace_id()).into_response()
            }
            Self::Repository(error) => error.into_response(),
        }
    }
}

impl From<WriteFailure> for ProblemDetails {
    fn from(failure: WriteFailure) -> Self {
        match failure {
            WriteFailure::Validation(failures) => {
                problem_details(failures, shared_kernel::errors::new_trace_id())
            }
            WriteFailure::Repository(error) => {
                error.into_problem(shared_kernel::errors::new_trace_id())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Customers
// ---------------------------------------------------------------------------

/// Port of `CustomerService.GetCustomerByIdAsync`.
pub(crate) async fn customer_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<CustomerApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "customer", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.customers.get_by_id(id).await },
            Some(entry_options(CUSTOMER_TAGS)),
        )
        .await
}

/// Port of `CustomerService.GetAllCustomersAsync`.
pub(crate) async fn all_customers(state: &AppState) -> RepositoryResult<Vec<CustomerApiModel>> {
    let key = state.cache_keys.compose(MODULE, "customer", VERSION, "all");

    let customers = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.customers.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(CUSTOMER_TAGS)),
        )
        .await?;

    Ok(customers.unwrap_or_default())
}

/// Port of `CustomerService.GetCustomersBySupportRepIdAsync`.
pub(crate) async fn customers_by_support_rep(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<CustomerApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "customer", VERSION, &format!("by-supportrep:{id}"));

    let customers = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state
                    .repositories
                    .customers
                    .get_by_support_rep_id(id)
                    .await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(CUSTOMER_TAGS)),
        )
        .await?;

    Ok(customers.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Employees
// ---------------------------------------------------------------------------

/// Port of `EmployeeService.GetEmployeeByIdAsync`.
pub(crate) async fn employee_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<EmployeeApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "employee", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async { state.repositories.employees.get_by_id(id).await },
            Some(entry_options(EMPLOYEE_TAGS)),
        )
        .await
}

/// Port of `EmployeeService.GetAllEmployeesAsync`.
pub(crate) async fn all_employees(state: &AppState) -> RepositoryResult<Vec<EmployeeApiModel>> {
    let key = state.cache_keys.compose(MODULE, "employee", VERSION, "all");

    let employees = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.employees.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(EMPLOYEE_TAGS)),
        )
        .await?;

    Ok(employees.unwrap_or_default())
}

/// Port of `EmployeeService.GetDirectReportsAsync`.
pub(crate) async fn direct_reports(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Vec<EmployeeApiModel>> {
    let key =
        state
            .cache_keys
            .compose(MODULE, "employee", VERSION, &format!("direct-reports:{id}"));

    let reports = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.employees.get_direct_reports(id).await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(EMPLOYEE_TAGS)),
        )
        .await?;

    Ok(reports.unwrap_or_default())
}

/// Port of `EmployeeService.GetReportsToAsync`.
///
/// Despite the route's name, this returns the employee with the given key
/// rather than that employee's manager: the C# service passes `id` straight to
/// `repo.GetReportsTo(id)`, which is a plain `FindAsync(id)`. Reproduced as
/// written, because the response is wire-visible.
pub(crate) async fn reports_to(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<EmployeeApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "employee", VERSION, &format!("reports-to:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let found = state.repositories.employees.get_reports_to(id).await?;
                Ok(found.map(|employee| employee.convert()))
            },
            Some(entry_options(EMPLOYEE_TAGS)),
        )
        .await
}

// ---------------------------------------------------------------------------
// Media types
// ---------------------------------------------------------------------------

/// Port of `MediaTypeService.GetMediaTypeByIdAsync`.
pub(crate) async fn media_type_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<MediaTypeApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "mediatype", VERSION, &format!("by-id:{id}"));

    state
        .cache
        .try_get_or_add(
            &key,
            || async {
                // The repository returns the entity here; the service converts.
                let found = state.repositories.media_types.get_by_id(id).await?;
                Ok(found.map(|media_type| media_type.convert()))
            },
            Some(entry_options(MEDIA_TYPE_TAGS)),
        )
        .await
}

/// Port of `MediaTypeService.GetAllMediaTypesAsync`.
pub(crate) async fn all_media_types(state: &AppState) -> RepositoryResult<Vec<MediaTypeApiModel>> {
    let key = state
        .cache_keys
        .compose(MODULE, "mediatype", VERSION, "all");

    let media_types = state
        .cache
        .try_get_or_add(
            &key,
            || async {
                let entities = state.repositories.media_types.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(MEDIA_TYPE_TAGS)),
        )
        .await?;

    Ok(media_types.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Genres — the only write surface any endpoint reaches
// ---------------------------------------------------------------------------

/// Port of `GenreService.GetGenreByIdAsync`.
pub(crate) async fn genre_by_id(
    state: &AppState,
    id: i32,
) -> RepositoryResult<Option<GenreApiModel>> {
    state
        .cache
        .try_get_or_add(
            &genre_key(state, &format!("by-id:{id}")),
            || async {
                // The repository returns the entity here; the service converts.
                let found = state.repositories.genres.get_by_id(id).await?;
                Ok(found.map(|genre| genre.convert()))
            },
            Some(entry_options(GENRE_TAGS)),
        )
        .await
}

/// Port of `GenreService.GetAllGenresAsync`.
pub(crate) async fn all_genres(state: &AppState) -> RepositoryResult<Vec<GenreApiModel>> {
    let genres = state
        .cache
        .try_get_or_add(
            &genre_key(state, "all"),
            || async {
                let entities = state.repositories.genres.get_all().await?;
                Ok(Some(convert_all(&entities)))
            },
            Some(entry_options(GENRE_TAGS)),
        )
        .await?;

    Ok(genres.unwrap_or_default())
}

/// Port of `GenreService.CreateGenreAsync`.
///
/// Validates a model built from the name alone — so the `Id` is zero and only
/// the name rules can fire — then inserts and invalidates.
pub(crate) async fn create_genre(
    state: &AppState,
    name: Option<String>,
) -> Result<GenreApiModel, WriteFailure> {
    let model = GenreApiModel {
        id: 0,
        name,
        tracks: Vec::new(),
    };

    model.validate().map_err(WriteFailure::Validation)?;

    let created = state
        .repositories
        .genres
        .add(Genre {
            id: 0,
            name: model.name.clone(),
        })
        .await?;

    invalidate_genres(state, None).await;

    Ok(created.convert())
}

/// Port of `GenreService.UpdateGenreAsync`.
///
/// Reports `false` when no such row exists, which is what lets the endpoint
/// answer 404 rather than 500.
pub(crate) async fn update_genre(
    state: &AppState,
    id: i32,
    name: Option<String>,
) -> Result<bool, WriteFailure> {
    let model = GenreApiModel {
        id,
        name,
        tracks: Vec::new(),
    };

    model.validate().map_err(WriteFailure::Validation)?;

    let updated = state
        .repositories
        .genres
        .update(Genre {
            id,
            name: model.name.clone(),
        })
        .await?;

    if updated {
        invalidate_genres(state, Some(id)).await;
    }

    Ok(updated)
}

/// Port of `GenreService.DeleteGenreAsync`.
///
/// Runs no validation and makes no existence check of its own — it relies
/// entirely on the repository reporting `false` for a missing row.
pub(crate) async fn delete_genre(state: &AppState, id: i32) -> RepositoryResult<bool> {
    let deleted = state.repositories.genres.delete(id).await?;

    if deleted {
        invalidate_genres(state, Some(id)).await;
    }

    Ok(deleted)
}

fn genre_key(state: &AppState, discriminator: &str) -> shared_kernel::caching::CacheKey {
    state
        .cache_keys
        .compose(MODULE, "genre", VERSION, discriminator)
}

/// Drops the genre entries a write invalidates.
///
/// The C# service calls `RemoveByTagAsync(GenreTags[0])` and, on update and
/// delete, also removes the specific by-id key. Both calls are reproduced —
/// but the tag call is the one that matters, and in the original it does
/// nothing at all, so there the `all` entry survives a write for the rest of
/// its twenty minutes. Here it is really dropped (F1 in the plan).
async fn invalidate_genres(state: &AppState, id: Option<i32>) {
    state.cache.remove_by_tag(GENRE_TAGS[0]).await;

    if let Some(id) = id {
        state
            .cache
            .remove(&genre_key(state, &format!("by-id:{id}")))
            .await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use shared_kernel::caching::CacheKeyComposer;

    #[test]
    fn the_cache_keys_match_the_originals() {
        let composer = CacheKeyComposer::from_parts("mmapi", "test");

        for (entity, discriminator, expected) in [
            (
                "customer",
                "by-supportrep:3",
                "test:mmapi:administration:customer:v1::::by-supportrep:3",
            ),
            (
                "employee",
                "direct-reports:2",
                "test:mmapi:administration:employee:v1::::direct-reports:2",
            ),
            (
                "employee",
                "reports-to:2",
                "test:mmapi:administration:employee:v1::::reports-to:2",
            ),
            (
                "mediatype",
                "all",
                "test:mmapi:administration:mediatype:v1::::all",
            ),
            (
                "genre",
                "by-id:1",
                "test:mmapi:administration:genre:v1::::by-id:1",
            ),
        ] {
            assert_eq!(
                composer
                    .compose(MODULE, entity, VERSION, discriminator)
                    .to_string(),
                expected
            );
        }
    }

    #[test]
    fn a_blank_genre_name_fails_the_same_rule_the_original_checks() {
        let model = GenreApiModel {
            id: 0,
            name: None,
            tracks: Vec::new(),
        };

        let failures = model.validate().expect_err("a null name should fail");

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].property_name, "Name");
        assert_eq!(failures[0].message, "'Name' must not be empty.");
    }

    #[test]
    fn an_over_long_genre_name_reports_the_length_message() {
        let model = GenreApiModel {
            id: 0,
            name: Some("x".repeat(121)),
            tracks: Vec::new(),
        };

        let failures = model.validate().expect_err("121 characters should fail");

        assert_eq!(
            failures[0].message,
            "The length of 'Name' must be 120 characters or fewer. You entered 121 characters."
        );
    }

    #[test]
    fn a_validation_failure_becomes_the_originals_400() {
        let failures = GenreApiModel::default()
            .validate()
            .expect_err("should fail");
        let problem: ProblemDetails = WriteFailure::Validation(failures).into();

        let document = serde_json::to_value(&problem).unwrap();

        assert_eq!(document["status"], 400);
        assert_eq!(document["title"], "Request validation failed.");
        assert_eq!(
            document["errors"]["Name"],
            serde_json::json!(["'Name' must not be empty."])
        );
    }
}
