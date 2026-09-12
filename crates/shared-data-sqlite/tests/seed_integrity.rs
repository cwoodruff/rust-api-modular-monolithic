//! The bundled database is stock Chinook and stays that way.
//!
//! `data/chinook.db` is committed, and the application can write to it — the
//! Genre endpoints are a live write surface. That combination has already gone
//! wrong twice:
//!
//! - The original ships a second copy at its repository root carrying eight
//!   rows a test run left behind, including `Genre` 1 renamed from `Rock` to
//!   `Updated_<guid>`. It is committed in that state.
//! - This repository shipped a genre called `PhaseFiveProbe` for two commits,
//!   from a row created while probing the original and then copied in
//!   wholesale.
//!
//! Neither was caught by a test, because nothing asserted what the seed data
//! should contain. These tests do. A failure here means the committed database
//! has been modified — restore it rather than updating the numbers, unless the
//! seed genuinely changed on purpose.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use shared_data_sqlite::create_pool;
use sqlx::SqlitePool;

/// Every table, with the row count stock Chinook has.
const CENSUS: [(&str, i64); 11] = [
    ("Album", 347),
    ("Artist", 275),
    ("Customer", 59),
    ("Employee", 8),
    ("Genre", 25),
    ("Invoice", 458),
    ("InvoiceLine", 2662),
    ("MediaType", 5),
    ("Playlist", 18),
    ("PlaylistTrack", 8715),
    ("Track", 3503),
];

/// Shapes a leftover test row tends to take.
///
/// The eight in the original's polluted copy are named `Updated_`, `CacheTest_`,
/// `TestGenre_`, `Concurrent_` and `CharsetTest_`, each with a GUID suffix. The
/// one this repository shipped was `PhaseFiveProbe`.
const ARTIFACT_PATTERNS: [&str; 6] = [
    "Updated!_%",
    "%Test!_%",
    "Concurrent!_%",
    "%Probe%",
    "%Parity%",
    "%Renamed%",
];

fn bundled_database() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));

    loop {
        let candidate = current.join("data/chinook.db");
        if candidate.is_file() {
            return candidate;
        }
        current = current.parent().expect("data/chinook.db should be bundled");
    }
}

async fn pool() -> SqlitePool {
    create_pool(&bundled_database())
        .await
        .expect("the bundled database should open")
}

#[tokio::test]
async fn every_table_holds_exactly_the_stock_rows() {
    // Catches pollution of any table, not just the one entity that currently
    // has write endpoints.
    let pool = pool().await;

    for (table, expected) in CENSUS {
        let count: i64 = sqlx::query_scalar(&format!(r#"SELECT count(*) FROM "{table}""#))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("counting {table}: {error}"));

        assert_eq!(
            count, expected,
            "{table} holds {count} rows, stock Chinook holds {expected} — \
             the committed database has been modified"
        );
    }
}

#[tokio::test]
async fn the_rows_writes_can_reach_still_say_what_they_should() {
    // Genre is the only entity with write endpoints, so it is the one that has
    // actually been corrupted in practice — twice.
    let pool = pool().await;

    let genre: String = sqlx::query_scalar(r#"SELECT "Name" FROM "Genre" WHERE "Id" = 1"#)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        genre, "Rock",
        "Genre 1 should be Rock; the original's polluted copy has it renamed \
         to Updated_<guid>"
    );

    let highest: i64 = sqlx::query_scalar(r#"SELECT max("Id") FROM "Genre""#)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Note this catches rows that were inserted and *kept*, not ones inserted
    // and deleted again: `Genre.Id` is a plain `integer` primary key rather
    // than `AUTOINCREMENT`, so SQLite hands the freed key straight back. The
    // checksum below is what covers that case.
    assert_eq!(
        highest, 25,
        "the highest genre key is {highest} rather than 25 — rows were inserted \
         into the committed database"
    );
}

#[tokio::test]
async fn no_table_carries_a_row_that_looks_like_a_test_artifact() {
    let pool = pool().await;

    // The name columns a write endpoint could plausibly reach.
    let named = [
        ("Genre", "Name"),
        ("MediaType", "Name"),
        ("Playlist", "Name"),
        ("Artist", "Name"),
        ("Album", "Title"),
        ("Track", "Name"),
    ];

    for (table, column) in named {
        for pattern in ARTIFACT_PATTERNS {
            let found: Vec<String> = sqlx::query_scalar(&format!(
                r#"SELECT "{column}" FROM "{table}" WHERE "{column}" LIKE ? ESCAPE '!'"#
            ))
            .bind(pattern)
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|error| panic!("scanning {table}.{column}: {error}"));

            assert!(
                found.is_empty(),
                "{table}.{column} matches the artifact pattern {pattern}: {found:?} — \
                 a test or a manual probe wrote to the committed database"
            );
        }
    }
}

/// The checksum of the stock database, as committed.
///
/// Update this **only** when the seed data is meant to change, and say so in
/// the commit message. A surprise failure here means something wrote to the
/// committed file.
const PRISTINE_SHA256: &str = "bc1185936f4d905a1b435059f0acdf642e70cc1317ecb4c25d5d1ae47142eebb";

#[test]
fn the_committed_file_is_byte_for_byte_the_stock_database() {
    // The content checks above miss one case: a row inserted and then deleted
    // again restores every count, and SQLite reuses the freed key, so nothing
    // logical differs. The file bytes still change — the freelist moves — and
    // a write that leaves no logical trace is still a write to a committed
    // file, which is the habit worth catching.
    let bytes = std::fs::read(bundled_database()).expect("the database should be readable");
    let digest = sha256(&bytes);

    assert_eq!(
        digest, PRISTINE_SHA256,
        "data/chinook.db has changed. If that was deliberate, update \
         PRISTINE_SHA256 and say why; otherwise restore the file — something \
         wrote to the committed database"
    );
}

/// A small SHA-256, so this test needs no dependency of its own.
fn sha256(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    let mut hash: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let mut message = bytes.to_vec();
    let bit_length = (bytes.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0_u32; 64];
        for (index, word) in chunk.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    hash.iter().map(|word| format!("{word:08x}")).collect()
}

#[tokio::test]
async fn the_bundled_file_is_the_one_the_original_host_reads() {
    // The original ships two copies. The host opens the one under its content
    // root, and that is the one this repository bundles; the other carries the
    // leftovers. Size alone does not distinguish them — both are 770,048
    // bytes — so this checks the contents.
    let pool = pool().await;

    let anchors = [
        ("Artist", "AC/DC"),
        ("MediaType", "MPEG audio file"),
        ("Playlist", "Music"),
    ];

    for (table, expected) in anchors {
        let column = if table == "Artist" || table == "MediaType" || table == "Playlist" {
            "Name"
        } else {
            "Title"
        };

        let value: String = sqlx::query_scalar(&format!(
            r#"SELECT "{column}" FROM "{table}" WHERE "Id" = 1"#
        ))
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(value, expected, "{table} 1 should be {expected}");
    }
}
