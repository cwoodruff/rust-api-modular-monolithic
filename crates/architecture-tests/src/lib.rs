//! Port of the C# `ModularMonolith.Architecture.Tests` project.
//!
//! The C# original used ArchUnitNET to assert rules about assembly references
//! and exported types. In Rust those same rules are structural: the crate graph
//! decides who can reference whom, and `pub(crate)` decides what a module
//! exports. This crate holds no code — the rules live in `tests/`, where they
//! read the real dependency graph from `cargo metadata`.
//!
//! - `module_boundaries.rs` — ports `ModuleBoundaryTests` (no module may
//!   reference another module) and `SharedKernelDependencyTests` (the shared
//!   crates are leaves and never reference a module).
//!
//! The third C# class, `PublicSurfaceTests`, has no runtime equivalent here:
//! Rust visibility enforces it at compile time.
