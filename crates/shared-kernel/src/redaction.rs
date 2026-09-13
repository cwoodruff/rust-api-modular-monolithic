//! Keeping secrets out of log records.
//!
//! `#[derive(Debug)]` is the default on every struct in this port, and a
//! derived `Debug` prints every field. That is what puts a password in a log
//! line: nobody logs `user.password`, but plenty of code logs `?user`, and a
//! `tracing` field written with `?` calls `Debug`. The same goes for a
//! refresh token in a request body, a private key in a persisted document, and
//! a connection string in the configuration.
//!
//! The types that hold one of those write their own `Debug` and substitute
//! [`REDACTED`], so the shape stays readable and the value never leaves the
//! process.

/// What a secret's value is printed as.
pub const REDACTED: &str = "<redacted>";

/// Reports whether a secret is set, without disclosing it.
///
/// Useful where the *presence* of a value is the thing worth debugging — an
/// optional key vault URI, an email that may or may not be configured.
#[must_use]
pub fn presence(value: Option<&str>) -> &'static str {
    if value.is_some_and(|value| !value.trim().is_empty()) {
        REDACTED
    } else {
        "<unset>"
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn presence_distinguishes_set_from_unset_without_disclosing_either() {
        assert_eq!(presence(Some("hunter2")), REDACTED);
        assert_eq!(presence(None), "<unset>");
        assert_eq!(presence(Some("   ")), "<unset>", "blank is not set");
    }
}
