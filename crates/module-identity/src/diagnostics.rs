//! Startup reporting on configured logins, ported from
//! `InMemoryUserStoreDiagnosticsHostedService`.
//!
//! The messages are upstream's, word for word, with its event ids. They exist
//! for one situation: a login that returns 401 and a developer who cannot see
//! why. Each usable entry is summarized — password length, never the password
//! — and each unusable one names its index and missing fields, so a split or
//! half-pasted account is visible at startup rather than at login.

use shared_kernel::Environment;

use crate::options::InMemoryUserRecord;

/// Fragments that suggest a shell paste joined a command onto the password.
const COMMAND_FRAGMENTS: [&str; 3] = ["user-secrets", "dotnet ", "--project"];

/// Why a password looks pasted, if it does. Port of `SuspiciousPasswordReason`.
#[must_use]
pub fn suspicious_password_reason(password: &str) -> Option<String> {
    let lowered = password.to_lowercase();
    if let Some(fragment) = COMMAND_FRAGMENTS
        .iter()
        .find(|fragment| lowered.contains(*fragment))
    {
        return Some(format!("contains command text ('{}')", fragment.trim()));
    }

    let whitespace = password.chars().filter(|c| c.is_whitespace()).count();
    (whitespace > 0).then(|| format!("contains {whitespace} whitespace character(s)"))
}

/// The required fields an entry is missing, in upstream's order.
fn missing_fields(record: &InMemoryUserRecord) -> Vec<&'static str> {
    [
        ("Username", record.username.as_str()),
        ("Password", record.password.as_str()),
        ("UserId", record.user_id.as_str()),
    ]
    .into_iter()
    .filter(|(_, value)| value.trim().is_empty())
    .map(|(field, _)| field)
    .collect()
}

fn joined_or_none(values: &[String]) -> String {
    let values: Vec<&str> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect();
    if values.is_empty() {
        "(none)".to_owned()
    } else {
        values.join(", ")
    }
}

/// Logs what upstream's diagnostics service logs at startup.
pub fn report(records: &[InMemoryUserRecord], environment: &Environment) {
    let configured = records.len();
    let name = environment.name();

    if !environment.exposes_operational_metadata() {
        if configured > 0 {
            tracing::warn!(
                event_id = 2001,
                "Identity:InMemoryUsers is configured with {configured} entries, but in-memory login is disabled in the {name} environment. Set ASPNETCORE_ENVIRONMENT=Development or Demo to enable it."
            );
        }
        return;
    }

    let mut valid = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let missing = missing_fields(record);
        if missing.is_empty() {
            valid.push((index, record));
        } else {
            tracing::warn!(
                event_id = 2006,
                "Ignoring Identity:InMemoryUsers:{index} because it is missing required field(s): {}. Keep Username, Password, UserId, Roles, Permissions, Email, and Tenant for one account on the same array index.",
                missing.join(", ")
            );
        }
    }

    if valid.is_empty() {
        if configured == 0 {
            tracing::warn!(
                event_id = 2002,
                "In-memory login is enabled for the {name} environment, but no Identity:InMemoryUsers entries are configured."
            );
        } else {
            tracing::warn!(
                event_id = 2003,
                "In-memory login is enabled for the {name} environment, but none of the {configured} configured Identity:InMemoryUsers entries are usable. Each entry must include Username, Password, and UserId."
            );
        }
        return;
    }

    let ignored = configured - valid.len();
    if ignored > 0 {
        tracing::warn!(
            event_id = 2004,
            "Loaded {} in-memory login users and ignored {ignored} incomplete Identity:InMemoryUsers entries. Each entry must include Username, Password, and UserId.",
            valid.len()
        );
    }

    for (index, record) in &valid {
        let tenant = record
            .tenant
            .as_deref()
            .map(str::trim)
            .filter(|tenant| !tenant.is_empty())
            .unwrap_or("(none)");
        tracing::info!(
            event_id = 2007,
            "Effective Identity:InMemoryUsers:{index} => Username='{}', UserId='{}', Roles='{}', Permissions='{}', Tenant='{tenant}', PasswordLength={}.",
            record.username.trim(),
            record.user_id.trim(),
            joined_or_none(&record.roles),
            joined_or_none(&record.permissions),
            record.password.chars().count()
        );
    }

    for (index, record) in &valid {
        if let Some(reason) = suspicious_password_reason(&record.password) {
            tracing::warn!(
                event_id = 2008,
                "The password for Identity:InMemoryUsers:{index} ({} characters) {reason}. A shell paste may have joined the next command to the value, and login will reject the password you type. Re-set it on its own line, quoted: dotnet user-secrets --project src/ModularMonolith.Api set \"Identity:InMemoryUsers:{index}:Password\" '<password>'",
                record.password.chars().count()
            );
        }
    }

    tracing::info!(
        event_id = 2005,
        "Loaded {} in-memory login user(s) for the {name} environment.",
        valid.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_command_is_named() {
        assert_eq!(
            suspicious_password_reason("hunter2dotnet user-secrets list").as_deref(),
            Some("contains command text ('user-secrets')")
        );
        assert_eq!(
            suspicious_password_reason("secret --project x").as_deref(),
            Some("contains command text ('--project')")
        );
    }

    #[test]
    fn whitespace_is_counted_when_no_command_is_found() {
        assert_eq!(
            suspicious_password_reason("correct horse").as_deref(),
            Some("contains 1 whitespace character(s)")
        );
        assert_eq!(suspicious_password_reason("Tr0ub4dor&3"), None);
    }

    #[test]
    fn missing_fields_are_named_in_order() {
        let record = InMemoryUserRecord {
            username: "demo".to_owned(),
            ..InMemoryUserRecord::default()
        };

        assert_eq!(missing_fields(&record), ["Password", "UserId"]);
    }
}
