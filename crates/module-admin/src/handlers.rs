//! Administration's endpoint handlers, ported from `Admin.Module/Endpoints/*Handlers`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use shared_kernel::ApiError;
use shared_kernel::data::found;
use shared_kernel::validation::Validated;

use crate::AdministrationState;
use crate::models::{
    CreateGenreRequest, CustomerApiModel, CustomerPurchasesApiModel, EmployeeApiModel,
    GenreApiModel, GenreUpdated, MediaTypeApiModel, UpdateGenreRequest,
};

/// What most handlers here return.
pub(crate) type Answer<T> = Result<Json<T>, ApiError>;

type Administration = State<Arc<AdministrationState>>;

// Customers

pub(crate) async fn get_customer_by_id(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<CustomerApiModel> {
    found(admin.customers.get_customer_by_id(id).await?)
}

pub(crate) async fn get_customer_purchases(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<CustomerPurchasesApiModel> {
    found(admin.customers.get_customer_purchases(id).await?)
}

pub(crate) async fn get_all_customers(
    State(admin): Administration,
) -> Answer<Vec<CustomerApiModel>> {
    Ok(Json(admin.customers.get_all_customers().await?))
}

pub(crate) async fn get_customers_by_support_rep_id(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<Vec<CustomerApiModel>> {
    Ok(Json(
        admin.customers.get_customers_by_support_rep_id(id).await?,
    ))
}

// Employees

pub(crate) async fn get_employee_by_id(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<EmployeeApiModel> {
    found(admin.employees.get_employee_by_id(id).await?)
}

pub(crate) async fn get_all_employees(
    State(admin): Administration,
) -> Answer<Vec<EmployeeApiModel>> {
    Ok(Json(admin.employees.get_all_employees().await?))
}

pub(crate) async fn get_employee_direct_reports(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<Vec<EmployeeApiModel>> {
    Ok(Json(admin.employees.get_direct_reports(id).await?))
}

pub(crate) async fn get_employee_reports_to(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<EmployeeApiModel> {
    found(admin.employees.get_reports_to(id).await?)
}

// Genres

pub(crate) async fn get_genre_by_id(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<GenreApiModel> {
    found(admin.genres.get_genre_by_id(id).await?)
}

pub(crate) async fn get_all_genres(State(admin): Administration) -> Answer<Vec<GenreApiModel>> {
    Ok(Json(admin.genres.get_all_genres().await?))
}

/// 201 with `Location: /api/admin/genres/{id}` and the PascalCase model.
pub(crate) async fn create_genre(
    State(admin): Administration,
    Validated(request): Validated<CreateGenreRequest>,
) -> Result<Response, ApiError> {
    let created = admin.genres.create_genre(request.name).await?;
    let location = format!("/api/admin/genres/{}", created.id);

    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(created),
    )
        .into_response())
}

/// 200 with the lowercase echo, or 404.
pub(crate) async fn update_genre(
    State(admin): Administration,
    Path(id): Path<i32>,
    Validated(request): Validated<UpdateGenreRequest>,
) -> Answer<GenreUpdated> {
    let name = request.name.clone();

    if admin.genres.update_genre(id, request.name).await? {
        Ok(Json(GenreUpdated { id, name }))
    } else {
        Err(ApiError::NotFound)
    }
}

/// 204, or 404 when there is nothing to delete.
pub(crate) async fn delete_genre(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Result<StatusCode, ApiError> {
    if admin.genres.delete_genre(id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

// Media types

pub(crate) async fn get_media_type_by_id(
    State(admin): Administration,
    Path(id): Path<i32>,
) -> Answer<MediaTypeApiModel> {
    found(admin.media_types.get_media_type_by_id(id).await?)
}

pub(crate) async fn get_all_media_types(
    State(admin): Administration,
) -> Answer<Vec<MediaTypeApiModel>> {
    Ok(Json(admin.media_types.get_all_media_types().await?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use async_trait::async_trait;
    use shared_kernel::DataResult;

    use super::*;
    use crate::services::{CustomerService, EmployeeService, GenreService, MediaTypeService};

    /// Genre 1 exists; nothing else does.
    struct Fake;

    #[async_trait]
    impl CustomerService for Fake {
        async fn get_customer_by_id(&self, _: i32) -> DataResult<Option<CustomerApiModel>> {
            Ok(None)
        }
        async fn get_all_customers(&self) -> DataResult<Vec<CustomerApiModel>> {
            Ok(Vec::new())
        }
        async fn get_customers_by_support_rep_id(
            &self,
            _: i32,
        ) -> DataResult<Vec<CustomerApiModel>> {
            Ok(Vec::new())
        }
        async fn get_customer_purchases(
            &self,
            id: i32,
        ) -> DataResult<Option<CustomerPurchasesApiModel>> {
            Ok((id == 1).then_some(CustomerPurchasesApiModel {
                customer_id: 1,
                total_spent: rust_decimal::Decimal::ZERO,
                invoice_count: 0,
                last_purchase_at: None,
            }))
        }
    }

    #[async_trait]
    impl EmployeeService for Fake {
        async fn get_employee_by_id(&self, _: i32) -> DataResult<Option<EmployeeApiModel>> {
            Ok(None)
        }
        async fn get_all_employees(&self) -> DataResult<Vec<EmployeeApiModel>> {
            Ok(Vec::new())
        }
        async fn get_direct_reports(&self, _: i32) -> DataResult<Vec<EmployeeApiModel>> {
            Ok(Vec::new())
        }
        async fn get_reports_to(&self, _: i32) -> DataResult<Option<EmployeeApiModel>> {
            Ok(None)
        }
    }

    #[async_trait]
    impl GenreService for Fake {
        async fn get_genre_by_id(&self, _: i32) -> DataResult<Option<GenreApiModel>> {
            Ok(None)
        }
        async fn get_all_genres(&self) -> DataResult<Vec<GenreApiModel>> {
            Ok(Vec::new())
        }
        async fn create_genre(&self, name: Option<String>) -> Result<GenreApiModel, ApiError> {
            Ok(GenreApiModel { id: 26, name })
        }
        async fn update_genre(&self, id: i32, _: Option<String>) -> Result<bool, ApiError> {
            Ok(id == 1)
        }
        async fn delete_genre(&self, id: i32) -> Result<bool, ApiError> {
            Ok(id == 1)
        }
    }

    #[async_trait]
    impl MediaTypeService for Fake {
        async fn get_media_type_by_id(&self, _: i32) -> DataResult<Option<MediaTypeApiModel>> {
            Ok(None)
        }
        async fn get_all_media_types(&self) -> DataResult<Vec<MediaTypeApiModel>> {
            Ok(Vec::new())
        }
    }

    fn admin() -> Administration {
        let fake = Arc::new(Fake);
        State(Arc::new(AdministrationState::from_services(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
        )))
    }

    #[tokio::test]
    async fn delete_genre_answers_no_content_or_not_found() {
        assert_eq!(
            delete_genre(admin(), Path(1)).await.unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(matches!(
            delete_genre(admin(), Path(2)).await,
            Err(ApiError::NotFound)
        ));
    }

    #[tokio::test]
    async fn update_genre_echoes_the_id_and_name() {
        let request = Validated(UpdateGenreRequest {
            name: Some("Jazz".to_owned()),
        });
        let Json(echo) = update_genre(admin(), Path(1), request).await.unwrap();

        assert_eq!(
            echo,
            GenreUpdated {
                id: 1,
                name: Some("Jazz".to_owned())
            }
        );
    }

    #[tokio::test]
    async fn create_genre_answers_created_with_its_location() {
        let request = Validated(CreateGenreRequest {
            name: Some("Jazz".to_owned()),
        });
        let response = create_genre(admin(), request).await.unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[header::LOCATION], "/api/admin/genres/26");
    }

    #[tokio::test]
    async fn purchases_answer_ok_or_not_found() {
        assert!(get_customer_purchases(admin(), Path(1)).await.is_ok());
        assert!(matches!(
            get_customer_purchases(admin(), Path(2)).await,
            Err(ApiError::NotFound)
        ));
    }
}
