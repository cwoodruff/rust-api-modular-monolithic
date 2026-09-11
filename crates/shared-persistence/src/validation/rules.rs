//! FluentValidation's rule semantics and message templates.
//!
//! # The rule that governs every other rule
//!
//! In FluentValidation, **only `NotNull` fails on a null value**. Every other
//! built-in validator — `MaximumLength`, `Matches`, `EmailAddress`,
//! `GreaterThan`, `LessThanOrEqualTo` — returns valid when the value is null
//! and only then checks anything.
//!
//! That is not a detail. It is the difference between "optional, but validated
//! when present" and "required", and the original leans on it constantly:
//! `Customer.Email` has an `EmailAddress()` rule and no `NotNull()`, so a
//! customer with no email is valid while a customer with `"nope"` is not. Every
//! rule below reproduces it.
//!
//! # Messages
//!
//! Messages are wire-visible: they land in the `errors` map of the 400
//! response. They are reproduced here from FluentValidation's default English
//! templates, with `{PropertyName}` resolved the way FluentValidation resolves
//! it — by splitting the property name on case, so `FirstName` reads as
//! `First Name`. The `errors` map key keeps the original, unsplit name.
//!
//! These templates are the one part of this module taken from documentation
//! rather than from the repository, so Phase 8's parity run against the live
//! C# service is what finally confirms them.

use std::fmt::Display;

use regex::Regex;

/// A single rule failure, equivalent to `FluentValidation.Results.ValidationFailure`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationFailure {
    /// The property that failed, unsplit — this becomes the `errors` map key.
    pub property_name: String,
    /// The human-readable message.
    pub message: String,
}

impl ValidationFailure {
    /// Builds a failure for `property` with an already-rendered message.
    pub fn new(property: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            property_name: property.into(),
            message: message.into(),
        }
    }
}

/// Something that can validate itself, equivalent to having an
/// `AbstractValidator<T>` registered for it.
///
/// The C# services inject `IValidator<TModel>` so the container can supply it.
/// Here the rules live with the model, which removes the indirection without
/// changing when validation runs: the service still calls it before a write,
/// and a failure still surfaces as a 400.
pub trait Validate {
    /// Runs every rule, collecting all failures rather than stopping at the
    /// first — FluentValidation's default `Continue` cascade mode.
    ///
    /// # Errors
    ///
    /// Returns every failed rule.
    fn validate(&self) -> Result<(), Vec<ValidationFailure>>;
}

/// Accumulates failures while a validator runs its rules.
#[derive(Debug, Default)]
pub struct Rules {
    failures: Vec<ValidationFailure>,
}

impl Rules {
    /// Starts an empty rule run.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Finishes the run, yielding every failure collected.
    ///
    /// # Errors
    ///
    /// Returns the failures if any rule failed.
    pub fn finish(self) -> Result<(), Vec<ValidationFailure>> {
        if self.failures.is_empty() {
            Ok(())
        } else {
            Err(self.failures)
        }
    }

    fn fail(&mut self, property: &str, message: String) {
        self.failures
            .push(ValidationFailure::new(property, message));
    }

    /// `RuleFor(x => x.Prop).NotNull()` — the only rule a null value fails.
    pub fn not_null<T>(&mut self, property: &str, value: Option<&T>) -> &mut Self {
        if value.is_none() {
            self.fail(property, not_null_message(&split_pascal_case(property)));
        }
        self
    }

    /// `MaximumLength(max)`.
    pub fn max_length(&mut self, property: &str, value: Option<&str>, max: usize) -> &mut Self {
        if let Some(text) = value {
            let length = text.chars().count();
            if length > max {
                self.fail(
                    property,
                    max_length_message(&split_pascal_case(property), max, length),
                );
            }
        }
        self
    }

    /// `MinimumLength(min)`.
    pub fn min_length(&mut self, property: &str, value: Option<&str>, min: usize) -> &mut Self {
        if let Some(text) = value {
            let length = text.chars().count();
            if length < min {
                self.fail(
                    property,
                    min_length_message(&split_pascal_case(property), min, length),
                );
            }
        }
        self
    }

    /// `EmailAddress()`, in FluentValidation's ASP.NET Core-compatible mode.
    pub fn email(&mut self, property: &str, value: Option<&str>) -> &mut Self {
        if let Some(text) = value
            && !email_pattern().is_match(text)
        {
            self.fail(property, email_message(&split_pascal_case(property)));
        }
        self
    }

    /// `Matches(pattern)`.
    pub fn matches(&mut self, property: &str, value: Option<&str>, pattern: &Regex) -> &mut Self {
        if let Some(text) = value
            && !pattern.is_match(text)
        {
            self.fail(property, regex_message(&split_pascal_case(property)));
        }
        self
    }

    /// `GreaterThan(threshold)`.
    pub fn greater_than<T>(&mut self, property: &str, value: Option<T>, threshold: T) -> &mut Self
    where
        T: PartialOrd + Display,
    {
        if let Some(actual) = value
            && actual <= threshold
        {
            self.fail(
                property,
                greater_than_message(&split_pascal_case(property), &threshold.to_string()),
            );
        }
        self
    }

    /// `LessThanOrEqualTo(threshold)`.
    pub fn less_than_or_equal<T>(
        &mut self,
        property: &str,
        value: Option<T>,
        threshold: T,
    ) -> &mut Self
    where
        T: PartialOrd + Display,
    {
        if let Some(actual) = value
            && actual > threshold
        {
            self.fail(
                property,
                less_than_or_equal_message(&split_pascal_case(property), &threshold.to_string()),
            );
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Message templates
// ---------------------------------------------------------------------------

fn not_null_message(display: &str) -> String {
    format!("'{display}' must not be empty.")
}

fn max_length_message(display: &str, max: usize, actual: usize) -> String {
    format!(
        "The length of '{display}' must be {max} characters or fewer. You entered {actual} characters."
    )
}

fn min_length_message(display: &str, min: usize, actual: usize) -> String {
    format!(
        "The length of '{display}' must be at least {min} characters. You entered {actual} characters."
    )
}

fn email_message(display: &str) -> String {
    format!("'{display}' is not a valid email address.")
}

fn regex_message(display: &str) -> String {
    format!("'{display}' is not in the correct format.")
}

fn greater_than_message(display: &str, comparison: &str) -> String {
    format!("'{display}' must be greater than '{comparison}'.")
}

fn less_than_or_equal_message(display: &str, comparison: &str) -> String {
    format!("'{display}' must be less than or equal to '{comparison}'.")
}

/// Port of FluentValidation's `SplitPascalCase`, used to turn a property name
/// into the display name its messages quote.
///
/// A space goes before an uppercase letter when the previous character is not
/// uppercase *and* it is not the second character, or when the next character
/// is not uppercase — which keeps runs of capitals such as `ID` together. The
/// leading space the rule produces is trimmed.
#[must_use]
pub fn split_pascal_case(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }

    let characters: Vec<char> = name.chars().collect();
    let mut result = String::with_capacity(name.len() + 5);

    for (index, &current) in characters.iter().enumerate() {
        if current.is_uppercase() {
            let previous_is_lower = index > 1 && !characters[index - 1].is_uppercase();
            let next_is_lower = characters
                .get(index + 1)
                .is_some_and(|next| !next.is_uppercase());

            if previous_is_lower || next_is_lower {
                result.push(' ');
            }
        }

        result.push(current);
    }

    result.trim().to_owned()
}

/// Compiles a pattern that is known good at authoring time.
fn compiled(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => unreachable!("validator pattern `{pattern}` must compile: {error}"),
    }
}

/// The phone and fax pattern shared by the customer and employee validators.
///
/// Deliberately **unanchored**, exactly as written in the original, so it
/// matches a US-style number appearing anywhere in the value — `"call
/// 555-555-5555 now"` passes.
#[must_use]
pub fn phone_pattern() -> &'static Regex {
    static PATTERN: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| compiled(r"\(?\d{3}\)?[-\.]? *\d{3}[-\.]? *[-\.]?\d{4}"));
    &PATTERN
}

/// The postal code pattern, anchored and US-only.
///
/// This rejects the Canadian and European codes that fill the Chinook data, so
/// it only holds because nothing validates existing rows — validation runs on
/// writes alone.
#[must_use]
pub fn postal_code_pattern() -> &'static Regex {
    static PATTERN: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| compiled(r"^[0-9]{5}(?:-[0-9]{4})?$"));
    &PATTERN
}

/// FluentValidation's ASP.NET Core-compatible email check: one `@`, with
/// non-empty, whitespace-free text on both sides.
fn email_pattern() -> &'static Regex {
    static PATTERN: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| compiled(r"^[^@\s]+@[^@\s]+$"));
    &PATTERN
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn run(build: impl FnOnce(&mut Rules)) -> Vec<ValidationFailure> {
        let mut rules = Rules::new();
        build(&mut rules);
        rules.finish().err().unwrap_or_default()
    }

    #[test]
    fn only_not_null_rejects_a_missing_value() {
        // The governing semantic: every other rule passes on null.
        let failures = run(|rules| {
            rules
                .max_length("Title", None, 3)
                .min_length("Title", None, 100)
                .email("Email", None)
                .matches("Phone", None, phone_pattern())
                .greater_than("Bytes", None::<i32>, 0)
                .less_than_or_equal("UnitPrice", None::<i32>, 9);
        });

        assert!(
            failures.is_empty(),
            "no rule but NotNull should fail on null, got {failures:?}"
        );

        let required = run(|rules| {
            rules.not_null("Title", None::<&String>);
        });
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].message, "'Title' must not be empty.");
    }

    #[test]
    fn every_failing_rule_is_reported_not_just_the_first() {
        // FluentValidation's default cascade mode is Continue.
        let failures = run(|rules| {
            rules
                .min_length("Title", Some("ab"), 3)
                .max_length("Title", Some("ab"), 1);
        });

        assert_eq!(failures.len(), 2);
    }

    #[test]
    fn messages_match_the_fluentvalidation_templates() {
        let failures = run(|rules| {
            rules
                .max_length("FirstName", Some("abcdef"), 3)
                .min_length("LastName", Some("ab"), 5)
                .email("Email", Some("nope"))
                .matches("Phone", Some("nope"), phone_pattern())
                .greater_than("Quantity", Some(0), 0)
                .less_than_or_equal("UnitPrice", Some(10), 9);
        });

        let messages: Vec<&str> = failures.iter().map(|f| f.message.as_str()).collect();

        assert_eq!(
            messages,
            vec![
                "The length of 'First Name' must be 3 characters or fewer. You entered 6 characters.",
                "The length of 'Last Name' must be at least 5 characters. You entered 2 characters.",
                "'Email' is not a valid email address.",
                "'Phone' is not in the correct format.",
                "'Quantity' must be greater than '0'.",
                "'Unit Price' must be less than or equal to '9'.",
            ]
        );
    }

    #[test]
    fn the_failure_key_keeps_the_unsplit_property_name() {
        // The message reads "First Name" but the errors map is keyed
        // "FirstName", which is what a client matches against.
        let failures = run(|rules| {
            rules.not_null("FirstName", None::<&String>);
        });

        assert_eq!(failures[0].property_name, "FirstName");
        assert!(failures[0].message.contains("'First Name'"));
    }

    #[test]
    fn property_names_split_the_way_fluentvalidation_splits_them() {
        assert_eq!(split_pascal_case("FirstName"), "First Name");
        assert_eq!(split_pascal_case("Title"), "Title");
        assert_eq!(split_pascal_case("ArtistId"), "Artist Id");
        assert_eq!(split_pascal_case("UnitPrice"), "Unit Price");
        assert_eq!(
            split_pascal_case("BillingPostalCode"),
            "Billing Postal Code"
        );
        assert_eq!(split_pascal_case("Id"), "Id");
        assert_eq!(split_pascal_case(""), "");
    }

    #[test]
    fn boundary_lengths_are_inclusive() {
        let failures = run(|rules| {
            rules
                .max_length("Title", Some("abc"), 3)
                .min_length("Title", Some("abc"), 3);
        });

        assert!(failures.is_empty(), "the boundary itself is valid");
    }

    #[test]
    fn the_phone_pattern_is_unanchored_as_written() {
        // A quirk of the original worth keeping visible: the pattern has no
        // anchors, so it matches a number embedded in other text.
        assert!(phone_pattern().is_match("555-555-5555"));
        assert!(phone_pattern().is_match("(555) 555-5555"));
        assert!(phone_pattern().is_match("call 555.555.5555 today"));
        assert!(!phone_pattern().is_match("not a phone number"));
    }

    #[test]
    fn the_postal_pattern_accepts_only_us_codes() {
        assert!(postal_code_pattern().is_match("12345"));
        assert!(postal_code_pattern().is_match("12345-6789"));
        assert!(!postal_code_pattern().is_match("K1A 0B1"), "Canadian");
        assert!(!postal_code_pattern().is_match("SW1A 1AA"), "British");
        assert!(!postal_code_pattern().is_match("1234"));
    }

    #[test]
    fn email_checks_for_one_at_sign_with_text_either_side() {
        for valid in ["a@b", "user@example.com", "user+tag@example.co.uk"] {
            let failures = run(|rules| {
                rules.email("Email", Some(valid));
            });
            assert!(failures.is_empty(), "{valid} should be accepted");
        }

        for invalid in [
            "nope",
            "@example.com",
            "user@",
            "a@b@c",
            "user name@example.com",
            "",
        ] {
            let failures = run(|rules| {
                rules.email("Email", Some(invalid));
            });
            assert_eq!(failures.len(), 1, "{invalid:?} should be rejected");
        }
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // "Gonçalves" is 9 characters; a byte count would make it 10 and could
        // trip a maximum the original would not.
        let failures = run(|rules| {
            rules.max_length("LastName", Some("Gonçalves"), 9);
        });

        assert!(failures.is_empty());
    }
}
