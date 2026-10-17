//! Reporting on its own host, reading as the read-only login. Port of
//! upstream's `ReportingTests` and `ReportingEndpointsTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use identity_contracts::permissions::{CATALOG_READ, REPORT_VIEW};
use module_tests::{Data, ModuleTestHost, Which};
use test_support::auth::TestUser;
use test_support::http::TestRequest;

fn viewer() -> TestUser {
    TestUser::with(&[REPORT_VIEW])
}

fn admin() -> TestUser {
    TestUser::with(&[]).admin()
}

#[tokio::test]
async fn sales_by_genre_sums_track_sales_per_genre() {
    let host = ModuleTestHost::start(Which::Reporting, Data::Seeded).await;
    let mut connection = host.connect().await;
    sqlx::raw_sql(r#"INSERT INTO catalog."TrackSales" VALUES (1, 5, now()), (2, 2, now())"#)
        .execute(&mut connection)
        .await
        .unwrap();

    let rows = host
        .send_as(&viewer(), TestRequest::get("/api/reporting/sales-by-genre"))
        .await
        .json();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 25, "every genre, sold or not");
    assert_eq!(rows[0]["GenreName"], "Rock", "ordered by units sold");
    assert_eq!(rows[0]["UnitsSold"], 7, "tracks 1 and 2 are both Rock");
    assert_eq!(rows[0]["TrackCount"], 1297);
}

#[tokio::test]
async fn invoice_lines_carry_track_and_customer_names() {
    let host = ModuleTestHost::start(Which::Reporting, Data::Seeded).await;

    let lines = host
        .send_as(
            &viewer(),
            TestRequest::get("/api/reporting/invoices/1/lines"),
        )
        .await
        .json();
    assert_eq!(lines[0]["InvoiceId"], 1);
    assert_eq!(lines[0]["CustomerName"], "Hugh O'Reilly");
    assert!(lines[0]["TrackName"].is_string());

    assert_eq!(
        host.send_as(
            &viewer(),
            TestRequest::get("/api/reporting/invoices/999999/lines")
        )
        .await
        .status,
        404
    );
}

#[tokio::test]
async fn reports_need_report_view_and_integrity_needs_an_admin() {
    let host = ModuleTestHost::start(Which::Reporting, Data::Seeded).await;

    assert_eq!(
        host.send_as(
            &TestUser::with(&[CATALOG_READ]),
            TestRequest::get("/api/reporting/sales-by-genre")
        )
        .await
        .status,
        403
    );
    assert_eq!(
        host.send_as(&viewer(), TestRequest::post("/api/reporting/integrity/run"))
            .await
            .status,
        403
    );
    assert_eq!(
        host.send_as(
            &viewer(),
            TestRequest::get("/api/reporting/integrity/findings")
        )
        .await
        .status,
        403
    );
}

#[tokio::test]
async fn stock_data_has_no_orphans() {
    let host = ModuleTestHost::start(Which::Reporting, Data::Seeded).await;

    let run = host
        .send_as(&admin(), TestRequest::post("/api/reporting/integrity/run"))
        .await
        .json();
    assert_eq!(
        (
            run["Detected"].as_i64(),
            run["Resolved"].as_i64(),
            run["Open"].as_i64()
        ),
        (Some(0), Some(0), Some(0))
    );
    assert!(run["RanAt"].as_str().unwrap().ends_with("+00:00"));

    let findings = host
        .send_as(
            &admin(),
            TestRequest::get("/api/reporting/integrity/findings"),
        )
        .await
        .json();
    assert_eq!(findings, serde_json::json!([]));
}

#[tokio::test]
async fn an_orphan_is_found_once_and_resolved_when_its_reference_returns() {
    let host = ModuleTestHost::start(Which::Reporting, Data::Seeded).await;
    let mut connection = host.connect().await;
    let line: i32 = sqlx::query_scalar(r#"INSERT INTO orders."InvoiceLine" ("InvoiceId", "TrackId", "UnitPrice", "Quantity") VALUES (1, 999999, 0.99, 1) RETURNING "Id""#)
        .fetch_one(&mut connection)
        .await
        .unwrap();

    let first = host
        .send_as(&admin(), TestRequest::post("/api/reporting/integrity/run"))
        .await
        .json();
    assert_eq!(first["Detected"], 1);
    assert_eq!(first["Open"], 1);

    let again = host
        .send_as(&admin(), TestRequest::post("/api/reporting/integrity/run"))
        .await
        .json();
    assert_eq!(
        again["Detected"], 0,
        "an open finding is not recorded twice"
    );
    assert_eq!(again["Open"], 1);

    let findings = host
        .send_as(
            &admin(),
            TestRequest::get("/api/reporting/integrity/findings"),
        )
        .await
        .json();
    assert_eq!(findings[0]["CheckName"], "invoice-line-track");
    assert_eq!(findings[0]["SourceSchema"], "orders");
    assert_eq!(findings[0]["SourceTable"], "InvoiceLine");
    assert_eq!(findings[0]["SourceId"], line);
    assert_eq!(findings[0]["MissingReference"], "catalog.Track 999999");

    sqlx::query(r#"UPDATE orders."InvoiceLine" SET "TrackId" = 1 WHERE "Id" = $1"#)
        .bind(line)
        .execute(&mut connection)
        .await
        .unwrap();

    let resolved = host
        .send_as(&admin(), TestRequest::post("/api/reporting/integrity/run"))
        .await
        .json();
    assert_eq!(resolved["Resolved"], 1);
    assert_eq!(resolved["Open"], 0);
}

#[tokio::test]
async fn the_reporting_login_cannot_write_to_another_modules_tables() {
    let host = ModuleTestHost::start(Which::Reporting, Data::Seeded).await;
    let mut reader = host.connect_as_reporting().await;

    let refused = sqlx::query(r#"INSERT INTO catalog."Track" ("Name") VALUES ('nope')"#)
        .execute(&mut reader)
        .await
        .unwrap_err();
    let code = refused
        .as_database_error()
        .and_then(|error| error.code())
        .map(|code| code.into_owned());
    assert_eq!(
        code.as_deref(),
        Some("42501"),
        "permission denied: {refused}"
    );

    let readable: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM catalog."Track""#)
        .fetch_one(&mut reader)
        .await
        .unwrap();
    assert_eq!(readable, 3503, "but it can read them");
}

/// Port of upstream's `ViewColumnReader`: `FROM`/`JOIN schema."Table" alias`
/// and `alias."Column"`, read out of the view SQL.
fn view_references(sql: &str) -> BTreeSet<(String, String, String)> {
    let tokens: Vec<&str> = sql.split_whitespace().collect();
    let mut aliases: BTreeMap<String, (String, String)> = BTreeMap::new();

    for window in tokens.windows(3) {
        let keyword = window[0].to_ascii_uppercase();
        if (keyword == "FROM" || keyword == "JOIN")
            && let Some((schema, table)) = window[1].split_once('.')
            && table.starts_with('"')
        {
            aliases.insert(
                window[2].trim_end_matches(';').to_owned(),
                (schema.to_owned(), table.trim_matches('"').to_owned()),
            );
        }
    }

    let mut references = BTreeSet::new();
    let mut rest = sql;
    while let Some(index) = rest.find(".\"") {
        let before = &rest[..index];
        let alias: String = before
            .chars()
            .rev()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let after = &rest[index + 2..];
        let column = after.split('"').next().unwrap_or_default().to_owned();
        if let Some((schema, table)) = aliases.get(&alias) {
            references.insert((schema.clone(), table.clone(), column));
        }
        rest = &rest[index + 2..];
    }

    references
}

#[tokio::test]
async fn every_column_reporting_reads_still_exists_in_its_owners_schema() {
    let migrations =
        std::fs::read_dir(test_support::content_root().join("crates/module-reporting/migrations"))
            .unwrap();
    let mut references = BTreeSet::new();
    for migration in migrations {
        let sql = std::fs::read_to_string(migration.unwrap().path()).unwrap();
        for view in sql.split("CREATE VIEW").skip(1) {
            references.extend(view_references(view.split(';').next().unwrap()));
        }
    }
    assert!(
        references.len() >= 10,
        "the reader found too little to mean anything: {references:?}"
    );

    let host = ModuleTestHost::start(Which::Reporting, Data::Empty).await;
    let mut connection = host.connect().await;
    for (schema, table, column) in &references {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 AND column_name = $3)",
        )
        .bind(schema)
        .bind(table)
        .bind(column)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert!(
            exists,
            "{schema}.\"{table}\".\"{column}\" is read by a Reporting view but no longer exists"
        );
    }
}
