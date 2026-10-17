//! Administration's validators, ported from `Admin.Module/Validation`.
//!
//! Two layers, as upstream has them:
//!
//! - **The request filter.** `CreateGenreRequestValidator` and
//!   `UpdateGenreRequestValidator` (`NotEmpty`, `MaximumLength(120)`), adapted
//!   to the kernel's [`RequestValidator`] the way upstream's
//!   `FluentRequestValidator<T>` adapts FluentValidation. They run before the
//!   handler.
//! - **The service fallback.** `GenreValidator` (`NotNull`, `MaximumLength(120)`)
//!   runs inside the service and fails with the same 400 shape.

use shared_kernel::validation::{RequestValidator, ValidationErrors};
use validation_rules::{Rules, ValidationFailure, group};

use crate::models::{CreateGenreRequest, UpdateGenreRequest};

fn genre_request_rules(name: Option<&str>) -> Vec<ValidationFailure> {
    let mut rules = Rules::new();
    rules.not_empty("Name", name).max_length("Name", name, 120);
    rules.finish().err().unwrap_or_default()
}

/// Port of `CreateGenreRequestValidator`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CreateGenreRequestValidator;

impl RequestValidator<CreateGenreRequest> for CreateGenreRequestValidator {
    fn validate(&self, request: &CreateGenreRequest) -> ValidationErrors {
        group(&genre_request_rules(request.name.as_deref()))
    }
}

/// Port of `UpdateGenreRequestValidator`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UpdateGenreRequestValidator;

impl RequestValidator<UpdateGenreRequest> for UpdateGenreRequestValidator {
    fn validate(&self, request: &UpdateGenreRequest) -> ValidationErrors {
        group(&genre_request_rules(request.name.as_deref()))
    }
}

/// Port of `GenreValidator`, the service-level fallback.
pub(crate) fn validate_genre(name: Option<&str>) -> Result<(), ValidationErrors> {
    let mut rules = Rules::new();
    rules
        .not_null("Name", name.as_ref())
        .max_length("Name", name, 120);
    rules.finish().map_err(|failures| group(&failures))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_name_must_not_be_empty() {
        for name in [None, Some(""), Some("   ")] {
            let errors = CreateGenreRequestValidator.validate(&CreateGenreRequest {
                name: name.map(ToOwned::to_owned),
            });

            assert_eq!(errors["Name"], ["'Name' must not be empty."], "{name:?}");
        }
    }

    #[test]
    fn a_long_name_reports_its_length() {
        let errors = UpdateGenreRequestValidator.validate(&UpdateGenreRequest {
            name: Some("x".repeat(121)),
        });

        assert_eq!(
            errors["Name"],
            ["The length of 'Name' must be 120 characters or fewer. You entered 121 characters."]
        );
    }

    #[test]
    fn the_service_fallback_accepts_blank_but_not_null() {
        assert!(validate_genre(Some("")).is_ok());
        assert!(validate_genre(None).is_err());
    }
}
