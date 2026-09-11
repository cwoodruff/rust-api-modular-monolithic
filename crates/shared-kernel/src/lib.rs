//! Port of the C# `SharedKernel` project.
//!
//! This crate is the leaf of the dependency graph: it must never depend on a
//! module crate or on a persistence crate. The architecture tests enforce that.
//!
//! Lands in Phase 1 (see `docs/rust-translation-plan.md`):
//!
//! - `module` — the [`Module`] trait, port of the C# `IModule` contract.
//! - `config` — `appsettings.json` + environment layering, and the
//!   `Development` / `Demo` / `Production` gating that drives Swagger,
//!   health metadata, and the dev signing-key provider.
//! - `build_info` — port of `BuildInfoProvider`.
//! - `errors` — RFC 7807 ProblemDetails responses carrying a `traceId`.
//! - `caching` — the cache facade, key composer, TTL jitter, single flight, and
//!   a *working* tag index (the C# `RemoveByTagAsync` is a no-op; see F1).
//! - `traffic_control` — rate-limit partition keys and policy-name constants.

/// Environment names the C# host treats as special.
///
/// `Demo` is first-class in the original: it unlocks Swagger, health metadata,
/// and the development signing-key provider exactly like `Development` does.
pub mod environment {
    /// The variable the original reads, kept verbatim for drop-in configs.
    pub const ENV_VAR: &str = "ASPNETCORE_ENVIRONMENT";

    /// Development environment name.
    pub const DEVELOPMENT: &str = "Development";
    /// Demo environment name.
    pub const DEMO: &str = "Demo";
    /// Production environment name.
    pub const PRODUCTION: &str = "Production";
}
