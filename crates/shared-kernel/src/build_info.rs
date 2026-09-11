//! Port of `BuildInfoProvider`.
//!
//! The C# original reads an assembly's `AssemblyInformationalVersion` at
//! runtime, falling back to the assembly version and finally to `"1.0.0"`. Each
//! module's health endpoint reads its *own* assembly, which in practice always
//! matched the host because every project shares one version.
//!
//! Here every crate inherits `version.workspace = true`, so the workspace
//! version is that single value and the compiler supplies it — no reflection,
//! and no fallback chain to reproduce.

/// The service name used when `ServiceName` is not configured.
pub const DEFAULT_SERVICE_NAME: &str = "ModularMonolith.Api";

/// The app name the cache key composer falls back to.
///
/// Deliberately *not* [`DEFAULT_SERVICE_NAME`]: the C# composer defaults to
/// `"mmapi"` independently, so an unconfigured host composes keys under a
/// different name than it reports.
pub const DEFAULT_CACHE_APP_NAME: &str = "mmapi";

/// The environment the cache key composer falls back to when neither
/// `ASPNETCORE_ENVIRONMENT` nor `DOTNET_ENVIRONMENT` is set.
pub const DEFAULT_CACHE_ENVIRONMENT: &str = "prod";

/// Port of `BuildInfoProvider.GetInformationalVersion`.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_the_workspace_version() {
        // Every crate inherits `version.workspace = true`, so this is the same
        // value each module's health endpoint reports.
        assert_eq!(version(), "1.0.0");
    }
}
