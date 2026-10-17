//! Caching, ported from the C# `SharedKernel.Caching` namespace.
//!
//! The service layer reads through [`CacheFacade`] with keys built by
//! [`CacheKeyComposer`], exactly as the original does. See [`facade`] for the
//! two places this port deliberately behaves differently from the C# code.

pub mod facade;
pub mod key;
pub mod options;

pub use facade::CacheFacade;
pub use key::{CacheKey, CacheKeyComposer, CachePartitions};
pub use options::{CacheEntryOptions, CacheOptions};

/// The configuration section the cache binds, matching the original.
pub const CONFIGURATION_SECTION: &str = "Caching";

/// Where a module's entry limit is read: `Caching:Modules:<Module>:SizeLimit`.
#[must_use]
pub fn size_limit_key(module: &str) -> String {
    format!("{CONFIGURATION_SECTION}:Modules:{module}:SizeLimit")
}
