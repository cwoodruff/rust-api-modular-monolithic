//! Catalog's wire shapes, ported from `Catalog.Module/Models`.
//!
//! PascalCase, nulls written, collections empty rather than absent — the
//! original's serialization (P1). The members upstream removed when Catalog
//! stopped joining into Administration's and Orders' tables are gone here
//! too: `Genre`, `MediaType`, `InvoiceLines`, `GenreName` and `MediaTypeName`
//! (ADR-0005 to ADR-0007).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use shared_kernel::json::dotnet;

/// An artist, with their albums on the by-id route.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ArtistApiModel {
    pub id: i32,
    pub name: Option<String>,
    pub albums: Vec<AlbumApiModel>,
}

/// An album, with its artist and tracks on the by-id route.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct AlbumApiModel {
    pub id: i32,
    pub title: Option<String>,
    pub artist_name: Option<String>,
    pub artist_id: Option<i32>,
    pub artist: Option<ArtistApiModel>,
    pub tracks: Vec<TrackApiModel>,
}

/// A track. `Album` is always null and `Playlists` always empty, on every route.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TrackApiModel {
    pub id: i32,
    pub name: Option<String>,
    pub album_id: Option<i32>,
    pub media_type_id: Option<i32>,
    pub genre_id: Option<i32>,
    pub composer: Option<String>,
    pub milliseconds: Option<i32>,
    pub bytes: Option<i32>,
    pub unit_price: Option<Decimal>,
    pub album: Option<AlbumApiModel>,
    pub playlists: Vec<PlaylistApiModel>,
    pub album_name: Option<String>,
}

/// A playlist, with its tracks on the by-id route.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaylistApiModel {
    pub id: i32,
    pub name: Option<String>,
    pub tracks: Vec<TrackApiModel>,
}

/// Units sold, counted from `InvoiceFinalized` (ADR-0009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TrackSalesApiModel {
    pub track_id: i32,
    pub times_sold: i32,
    #[serde(with = "dotnet::utc_option")]
    pub last_sold_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_track_carries_exactly_upstreams_members_in_order() {
        let json = serde_json::to_string(&TrackApiModel::default()).unwrap();

        assert_eq!(
            json,
            r#"{"Id":0,"Name":null,"AlbumId":null,"MediaTypeId":null,"GenreId":null,"Composer":null,"Milliseconds":null,"Bytes":null,"UnitPrice":null,"Album":null,"Playlists":[],"AlbumName":null}"#
        );
    }

    #[test]
    fn an_unsold_track_reports_zero_and_a_null_date() {
        let sales = TrackSalesApiModel {
            track_id: 7,
            times_sold: 0,
            last_sold_at: None,
        };

        assert_eq!(
            serde_json::to_string(&sales).unwrap(),
            r#"{"TrackId":7,"TimesSold":0,"LastSoldAt":null}"#
        );
    }
}
