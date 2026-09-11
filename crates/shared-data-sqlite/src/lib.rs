//! Port of the C# `SharedKernel.DataSQLite` project.
//!
//! Only the host (`api`) may depend on this crate — module crates see the
//! repository *traits* from `shared-persistence` and nothing more. The
//! architecture tests enforce that edge.
//!
//! Lands in Phase 3 (see `docs/rust-translation-plan.md`). The C# original is
//! EF Core LINQ throughout, so there is no SQL to lift: every query is
//! re-derived here, keeping the deliberate query shapes the original chose —
//! split queries for the album/artist graphs, two lean queries for
//! playlist-by-id, an EXISTS subquery for tracks-by-invoice.

/// Connection-pool size, mirroring `AddDbContextPool(..., poolSize: 128)`.
///
/// SQLite serializes writes on a single writer regardless of pool size; the
/// read path is what benefits. Phase 3 also enables WAL journaling.
pub const MAX_POOL_CONNECTIONS: u32 = 128;
