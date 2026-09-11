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

/// Tag names each service invalidates after a write.
///
/// The original declares two tags per entity but only ever passes the first to
/// `RemoveByTagAsync`, so the `:by-id` variants are recorded and never used as
/// invalidation targets. They are listed here for fidelity.
pub mod tags {
    /// Tags for the Music module's entities.
    pub mod music {
        /// Album entries.
        pub const ALBUM: &str = "music:album";
        /// Artist entries.
        pub const ARTIST: &str = "music:artist";
        /// Track entries.
        pub const TRACK: &str = "music:track";
        /// Playlist entries.
        pub const PLAYLIST: &str = "music:playlist";
    }

    /// Tags for the Orders module's entities.
    pub mod orders {
        /// Invoice entries.
        pub const INVOICE: &str = "orders:invoice";
        /// Invoice line entries.
        pub const INVOICE_LINE: &str = "orders:invoiceline";
    }

    /// Tags for the Administration module's entities.
    pub mod administration {
        /// Customer entries.
        pub const CUSTOMER: &str = "administration:customer";
        /// Employee entries.
        pub const EMPLOYEE: &str = "administration:employee";
        /// Genre entries.
        pub const GENRE: &str = "administration:genre";
        /// Media type entries.
        pub const MEDIA_TYPE: &str = "administration:mediatype";
    }
}
