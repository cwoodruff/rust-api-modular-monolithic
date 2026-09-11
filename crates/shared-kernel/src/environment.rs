//! Host environment, ported from `IHostEnvironment` plus the checks
//! `BuildInfoProvider` performs on it.

use std::fmt;

/// The environment variable the C# host reads. Kept verbatim so an existing
/// deployment's configuration carries over unchanged.
pub const ENVIRONMENT_VARIABLE: &str = "ASPNETCORE_ENVIRONMENT";

/// Secondary variable the cache key composer falls back to.
pub const DOTNET_ENVIRONMENT_VARIABLE: &str = "DOTNET_ENVIRONMENT";

/// The environment the host is running as.
///
/// `Demo` is first-class in the original, not an afterthought: it unlocks
/// Swagger, the health endpoints' extra metadata, the in-memory user store, and
/// the development signing-key provider exactly as `Development` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Environment {
    /// The `Development` environment.
    Development,
    /// The `Demo` environment.
    Demo,
    /// The `Production` environment, and the default when nothing is set.
    Production,
    /// Any other name the host was started with.
    Other(String),
}

impl Environment {
    /// Reads the environment from the process, defaulting to `Production` the
    /// way ASP.NET Core does when `ASPNETCORE_ENVIRONMENT` is unset.
    #[must_use]
    pub fn from_process() -> Self {
        std::env::var(ENVIRONMENT_VARIABLE)
            .ok()
            .filter(|name| !name.trim().is_empty())
            .map_or(Self::Production, |name| Self::from_name(&name))
    }

    /// Parses an environment name. Matching is case-insensitive, mirroring
    /// `IHostEnvironment.IsEnvironment`, which compares with `OrdinalIgnoreCase`.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let trimmed = name.trim();

        if trimmed.eq_ignore_ascii_case("Development") {
            Self::Development
        } else if trimmed.eq_ignore_ascii_case("Demo") {
            Self::Demo
        } else if trimmed.eq_ignore_ascii_case("Production") {
            Self::Production
        } else {
            Self::Other(trimmed.to_owned())
        }
    }

    /// The environment's name, as the health and root endpoints report it.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Development => "Development",
            Self::Demo => "Demo",
            Self::Production => "Production",
            Self::Other(name) => name,
        }
    }

    /// Port of `IHostEnvironment.IsDevelopment()`.
    #[must_use]
    pub fn is_development(&self) -> bool {
        matches!(self, Self::Development)
    }

    /// Port of `BuildInfoProvider.ShouldExposeOperationalMetadata`.
    ///
    /// Gates Swagger, the version and service fields on the root and health
    /// endpoints, the in-memory user store, and the development key provider.
    #[must_use]
    pub fn exposes_operational_metadata(&self) -> bool {
        matches!(self, Self::Development | Self::Demo)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_is_case_insensitive_like_is_environment() {
        assert_eq!(
            Environment::from_name("development"),
            Environment::Development
        );
        assert_eq!(
            Environment::from_name("DEVELOPMENT"),
            Environment::Development
        );
        assert_eq!(Environment::from_name("Demo"), Environment::Demo);
        assert_eq!(
            Environment::from_name("  Production  "),
            Environment::Production
        );
    }

    #[test]
    fn unknown_names_are_preserved_verbatim() {
        let environment = Environment::from_name("Staging");

        assert_eq!(environment, Environment::Other("Staging".to_owned()));
        assert_eq!(environment.name(), "Staging");
    }

    #[test]
    fn only_development_and_demo_expose_operational_metadata() {
        assert!(Environment::Development.exposes_operational_metadata());
        assert!(Environment::Demo.exposes_operational_metadata());
        assert!(!Environment::Production.exposes_operational_metadata());
        assert!(!Environment::Other("Staging".to_owned()).exposes_operational_metadata());
    }

    #[test]
    fn demo_is_not_development() {
        // Demo shares Development's metadata exposure but is a distinct
        // environment; conflating the two would enable dev-only behavior.
        assert!(!Environment::Demo.is_development());
    }
}
