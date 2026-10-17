//! Request validation before the handler runs, ported from
//! `SharedKernel.Validation` (`ValidationFilter<T>`, `IRequestValidator<T>`).
//!
//! An endpoint opts in with
//! [`EndpointConventions::with_validation`](crate::endpoints::EndpointConventions::with_validation),
//! which is upstream's `.AddEndpointFilter<ValidationFilter<T>>()`. The handler
//! then takes a [`Validated<T>`]: the body is parsed and validated before the
//! handler's own code runs, and an invalid body answers the same 400 problem
//! document a service's `ValidationException` produces.
//!
//! The kernel defines the contract and never a rule engine: a module adapts
//! whatever it validates with — this port's FluentValidation-shaped rules — to
//! [`RequestValidator`]. Upstream keeps FluentValidation out of SharedKernel
//! for the same reason (ADR-0012).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;

use crate::JsonBody;
use crate::errors::{ApiError, CLIENT_ERROR_TYPE, VALIDATION_DETAIL, VALIDATION_TITLE};

/// The validation problem's title. Upstream's `ValidationFilter<T>.Title`.
pub const TITLE: &str = VALIDATION_TITLE;
/// The validation problem's detail.
pub const DETAIL: &str = VALIDATION_DETAIL;
/// The validation problem's type.
pub const TYPE: &str = CLIENT_ERROR_TYPE;

/// Per-field messages: empty when the request is valid.
pub type ValidationErrors = BTreeMap<String, Vec<String>>;

/// Checks one request type. Port of `IRequestValidator<TRequest>`.
pub trait RequestValidator<T>: Send + Sync {
    /// Errors keyed by field name, as the module names them.
    fn validate(&self, request: &T) -> ValidationErrors;
}

impl<T, F> RequestValidator<T> for F
where
    F: Fn(&T) -> ValidationErrors + Send + Sync,
{
    fn validate(&self, request: &T) -> ValidationErrors {
        self(request)
    }
}

/// The validator an endpoint registered, carried to the extractor.
pub(crate) struct ValidatorSlot<T>(pub(crate) Arc<dyn RequestValidator<T>>);

impl<T> Clone for ValidatorSlot<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// A body that parsed and passed its endpoint's validator.
#[derive(Debug, Clone)]
pub struct Validated<T>(pub T);

impl<T, S> FromRequest<S> for Validated<T>
where
    T: DeserializeOwned + Send + 'static,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let validator = request.extensions().get::<ValidatorSlot<T>>().cloned();
        let path = request.uri().path().to_owned();

        let JsonBody(body) = JsonBody::<T>::from_request(request, state).await?;

        // Fail closed, as upstream does: an endpoint that asks for validation
        // and has no validator is a programming error, not a pass.
        let Some(ValidatorSlot(validator)) = validator else {
            tracing::error!(
                "No IRequestValidator<{}> is registered for {path}.",
                short_type_name::<T>()
            );
            return Err(ApiError::Internal);
        };

        let errors = validator.validate(&body);

        if errors.is_empty() {
            Ok(Self(body))
        } else {
            Err(ApiError::Validation(errors))
        }
    }
}

/// `CreateGenreRequest` rather than `module_admin::models::CreateGenreRequest`.
fn short_type_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full)
}

/// One required-field rule, the shape Identity's validators use:
/// `'<field>' is required.` for each blank field, in order.
#[must_use]
pub fn required(fields: &[(&str, Option<&str>)]) -> ValidationErrors {
    fields
        .iter()
        .filter(|(_, value)| value.is_none_or(|value| value.trim().is_empty()))
        .map(|(field, _)| ((*field).to_owned(), vec![format!("'{field}' is required.")]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_missing_fields_are_both_required() {
        let errors = required(&[
            ("username", Some("  ")),
            ("password", None),
            ("other", Some("x")),
        ]);

        assert_eq!(errors.len(), 2);
        assert_eq!(errors["username"], ["'username' is required."]);
        assert_eq!(errors["password"], ["'password' is required."]);
    }
}
