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
//! - `route_guards.rs` — has no C# counterpart. Every route a module declares
//!   must reach a handler that asks for an `Authorized<P>`, or be named in that
//!   file as anonymous with the reason. `Authorized<P>` already makes it
//!   impossible to *reach* a caller without the policy passing; what it cannot
//!   catch is a handler that never asks for one, which compiles and serves the
//!   data to anyone.
//!
//! The third C# class, `PublicSurfaceTests`, has no runtime equivalent here:
//! Rust visibility enforces it at compile time.
