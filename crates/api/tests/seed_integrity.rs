//! Port of upstream's `SeedDatabaseIntegrityTests`: the seeded clone is the
//! Chinook data, untouched by any test, from the pinned script.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use sha2::{Digest, Sha256};
use sqlx::Connection;

const COUNTS: [(&str, i64); 11] = [
    (r#"administration."Genre""#, 25),
    (r#"administration."MediaType""#, 5),
    (r#"administration."Employee""#, 8),
    (r#"administration."Customer""#, 59),
    (r#"catalog."Artist""#, 275),
    (r#"catalog."Album""#, 347),
    (r#"catalog."Track""#, 3503),
    (r#"catalog."Playlist""#, 18),
    (r#"catalog."PlaylistTrack""#, 8715),
    (r#"orders."Invoice""#, 458),
    (r#"orders."InvoiceLine""#, 2662),
];

#[tokio::test]
async fn the_seed_is_the_chinook_data_and_nothing_else() {
    let host = common::host().await;
    let mut connection = test_support::postgres::server()
        .connect(&host.database)
        .await;

    for (table, expected) in COUNTS {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(count, expected, "{table}");
    }

    let rock: String =
        sqlx::query_scalar(r#"SELECT "Name" FROM administration."Genre" WHERE "Id" = 1"#)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(rock, "Rock");
    let max: i32 = sqlx::query_scalar(r#"SELECT MAX("Id") FROM administration."Genre""#)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(max, 25);

    let drafts: i64 =
        sqlx::query_scalar(r#"SELECT COUNT(*) FROM orders."Invoice" WHERE "Status" = 'Draft'"#)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        drafts, 458,
        "the migration's default makes every seeded invoice a draft"
    );

    for (table, column) in [
        ("administration.\"Genre\"", "Name"),
        ("administration.\"MediaType\"", "Name"),
        ("catalog.\"Playlist\"", "Name"),
        ("catalog.\"Artist\"", "Name"),
        ("catalog.\"Album\"", "Title"),
        ("catalog.\"Track\"", "Name"),
    ] {
        let polluted: i64 = sqlx::query_scalar(&format!(
            r#"SELECT COUNT(*) FROM {table} WHERE "{column}" LIKE 'Updated\_%' ESCAPE '\' OR "{column}" LIKE '%Test\_%' ESCAPE '\'
               OR "{column}" LIKE 'Concurrent\_%' ESCAPE '\' OR "{column}" LIKE '%Probe%'"#
        ))
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(polluted, 0, "{table}");
    }

    let mut transaction = connection.begin().await.unwrap();
    let next: i32 = sqlx::query_scalar(
        r#"INSERT INTO administration."Genre" ("Name") VALUES ('Probe') RETURNING "Id""#,
    )
    .fetch_one(&mut *transaction)
    .await
    .unwrap();
    assert_eq!(next, 26, "the seed advances the identity sequences");
    transaction.rollback().await.unwrap();
}

#[test]
fn the_seed_script_is_the_pinned_one() {
    let path = test_support::content_root().join("data/chinook-postgres-seed.sql");
    let text = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");

    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        hex,
        "2931dd5c4386dba191b0d0b9ca73f332a0aad89c8d1a58fdff702b9a79cd5029"
    );
}
