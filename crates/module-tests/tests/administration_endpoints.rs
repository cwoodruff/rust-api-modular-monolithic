//! Administration on its own host. Port of upstream's `AdministrationEndpointsTests`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use identity_contracts::permissions::{ADMINISTRATION_READ, ADMINISTRATION_WRITE};
use module_tests::{Data, ModuleTestHost, Which};
use serde_json::json;
use test_support::auth::TestUser;
use test_support::http::TestRequest;

fn reader() -> TestUser {
    TestUser::with(&[ADMINISTRATION_READ]).admin()
}

fn writer() -> TestUser {
    TestUser::with(&[ADMINISTRATION_READ, ADMINISTRATION_WRITE]).admin()
}

#[tokio::test]
async fn reads_need_the_role_the_permission_and_the_tenant() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    assert_eq!(
        host.send(TestRequest::get("/api/admin/customers"))
            .await
            .status,
        401
    );
    assert_eq!(
        host.send_as(
            &TestUser::with(&[ADMINISTRATION_READ]),
            TestRequest::get("/api/admin/customers")
        )
        .await
        .status,
        403
    );
    assert_eq!(
        host.send_as(
            &reader().asking_for_tenant("tenant-2"),
            TestRequest::get("/api/admin/customers")
        )
        .await
        .status,
        403
    );
    assert_eq!(
        host.send_as(
            &TestUser::with(&[ADMINISTRATION_READ]),
            TestRequest::get("/api/admin/customers/1/purchases")
        )
        .await
        .status,
        403,
        "purchases need the Admin role too"
    );
}

#[tokio::test]
async fn by_id_reads_keep_the_originals_partial_projections() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    let customer = host
        .send_as(&reader(), TestRequest::get("/api/admin/customers/1"))
        .await
        .json();
    assert_eq!(customer["FirstName"], "Luís");
    assert_eq!(customer["SupportRepId"], 3);
    assert_eq!(customer["SupportRepName"], "Jane Peacock");
    assert!(
        customer["Email"].is_null(),
        "P17: the by-id projection leaves these null"
    );
    assert!(
        customer.get("Invoices").is_none(),
        "Administration no longer joins into Orders"
    );

    let customers = host
        .send_as(&reader(), TestRequest::get("/api/admin/customers/"))
        .await
        .json();
    assert_eq!(customers.as_array().unwrap().len(), 59);
    assert_eq!(customers[0]["Email"], "luisg@embraer.com.br");
    assert!(
        customers[0]["SupportRepId"].is_null(),
        "the collection mapping drops it"
    );

    let employee = host
        .send_as(&reader(), TestRequest::get("/api/admin/employees/2"))
        .await
        .json();
    assert_eq!(employee["ReportsToNavigation"], "Andrew Adams");
    assert_eq!(employee["HireDate"], "2002-05-01T00:00:00Z");
    assert_eq!(employee["Customers"], json!([]));

    assert_eq!(
        host.send_as(&reader(), TestRequest::get("/api/admin/customers/999999"))
            .await
            .status,
        404
    );
    assert_eq!(
        host.send_as(&reader(), TestRequest::get("/api/admin/genres/abc"))
            .await
            .status,
        404
    );
    assert_eq!(
        host.send_as(&reader(), TestRequest::get("/api/admin/genres/999999"))
            .await
            .status,
        404
    );

    let genre = host
        .send_as(&reader(), TestRequest::get("/api/admin/genres/1"))
        .await
        .json();
    assert_eq!(genre, json!({ "Id": 1, "Name": "Rock" }));
}

#[tokio::test]
async fn reports_to_returns_the_manager_and_404_at_the_top() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    let manager = host
        .send_as(
            &reader(),
            TestRequest::get("/api/admin/employees/3/reports-to"),
        )
        .await
        .json();
    assert_eq!(manager["Id"], 2, "Jane Peacock reports to Nancy Edwards");

    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get("/api/admin/employees/1/reports-to")
        )
        .await
        .status,
        404
    );
    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get("/api/admin/employees/999999/reports-to")
        )
        .await
        .status,
        404
    );

    let reports = host
        .send_as(
            &reader(),
            TestRequest::get("/api/admin/employees/1/direct-reports"),
        )
        .await
        .json();
    assert!(
        reports
            .as_array()
            .unwrap()
            .iter()
            .all(|employee| employee["ReportsTo"] == 1)
    );
}

#[tokio::test]
async fn purchases_start_at_zero_for_a_known_customer() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    let purchases = host
        .send_as(
            &reader(),
            TestRequest::get("/api/admin/customers/1/purchases"),
        )
        .await
        .json();
    assert_eq!(purchases["CustomerId"], 1);
    assert_eq!(purchases["InvoiceCount"], 0);
    assert_eq!(purchases["TotalSpent"].as_f64(), Some(0.0));
    assert!(purchases["LastPurchaseAt"].is_null());

    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get("/api/admin/customers/999999/purchases")
        )
        .await
        .status,
        404
    );
}

#[tokio::test]
async fn a_genre_is_created_read_renamed_and_deleted() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    let created = host
        .send_as(
            &writer(),
            TestRequest::post("/api/admin/genres").json(&json!({ "Name": "Synthwave" })),
        )
        .await;
    assert_eq!(created.status, 201);
    let id = created.json()["Id"].as_i64().unwrap();
    assert_eq!(id, 26, "the seed's sequence was reset past its 25 genres");
    assert_eq!(
        created.header("location"),
        Some(format!("/api/admin/genres/{id}"))
    );

    let read = host
        .send_as(
            &reader(),
            TestRequest::get(&format!("/api/admin/genres/{id}")),
        )
        .await
        .json();
    assert_eq!(read["Name"], "Synthwave");

    let all = host
        .send_as(&reader(), TestRequest::get("/api/admin/genres"))
        .await
        .json();
    assert_eq!(
        all.as_array().unwrap().len(),
        26,
        "the list cache was invalidated by the create (F1)"
    );

    let renamed = host
        .send_as(
            &writer(),
            TestRequest::put(&format!("/api/admin/genres/{id}")).json(&json!({ "name": "Outrun" })),
        )
        .await;
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.json(), json!({ "id": id, "name": "Outrun" }));

    let reread = host
        .send_as(
            &reader(),
            TestRequest::get(&format!("/api/admin/genres/{id}")),
        )
        .await
        .json();
    assert_eq!(reread["Name"], "Outrun");

    assert_eq!(
        host.send_as(
            &writer(),
            TestRequest::put("/api/admin/genres/999999").json(&json!({ "Name": "x" }))
        )
        .await
        .status,
        404
    );

    assert_eq!(
        host.send_as(
            &writer(),
            TestRequest::delete(&format!("/api/admin/genres/{id}"))
        )
        .await
        .status,
        204
    );
    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::get(&format!("/api/admin/genres/{id}"))
        )
        .await
        .status,
        404
    );
    assert_eq!(
        host.send_as(
            &writer(),
            TestRequest::delete(&format!("/api/admin/genres/{id}"))
        )
        .await
        .status,
        404
    );
}

#[tokio::test]
async fn writes_need_the_write_permission() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    assert_eq!(
        host.send(TestRequest::post("/api/admin/genres").json(&json!({ "Name": "x" })))
            .await
            .status,
        401
    );
    assert_eq!(
        host.send_as(
            &reader(),
            TestRequest::post("/api/admin/genres").json(&json!({ "Name": "x" }))
        )
        .await
        .status,
        403
    );
}

#[tokio::test]
async fn an_invalid_name_is_refused_before_the_handler_runs() {
    let host = ModuleTestHost::start(Which::Administration, Data::Seeded).await;

    for body in [
        json!({}),
        json!({ "Name": null }),
        json!({ "Name": "   " }),
        json!({ "Name": "x".repeat(121) }),
    ] {
        let refused = host
            .send_as(
                &writer(),
                TestRequest::post("/api/admin/genres").json(&body),
            )
            .await;
        assert_eq!(refused.status, 400, "{body}");
        assert_eq!(
            refused.header("content-type").as_deref(),
            Some("application/problem+json")
        );
        let problem = refused.json();
        assert_eq!(problem["title"], "Request validation failed.");
        assert_eq!(
            problem["errors"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["Name"],
            "{body}"
        );
    }

    let too_long = host
        .send_as(
            &writer(),
            TestRequest::post("/api/admin/genres").json(&json!({ "Name": "x".repeat(121) })),
        )
        .await
        .json();
    assert_eq!(
        too_long["errors"]["Name"][0],
        "The length of 'Name' must be 120 characters or fewer. You entered 121 characters."
    );

    let update = host
        .send_as(
            &writer(),
            TestRequest::put("/api/admin/genres/1").json(&json!({ "Name": "" })),
        )
        .await;
    assert_eq!(update.status, 400);
    assert_eq!(
        update.json()["errors"]["Name"][0],
        "'Name' must not be empty."
    );

    let count = host
        .send_as(&reader(), TestRequest::get("/api/admin/genres"))
        .await
        .json();
    assert_eq!(
        count.as_array().unwrap().len(),
        25,
        "no refused body reached the database"
    );
}

#[tokio::test]
async fn a_content_type_with_a_charset_is_accepted_and_concurrent_creates_get_distinct_ids() {
    let host =
        std::sync::Arc::new(ModuleTestHost::start(Which::Administration, Data::Seeded).await);

    let charset = host
        .send_as(
            &writer(),
            TestRequest::post("/api/admin/genres")
                .raw("application/json; charset=utf-8", r#"{"Name":"Charset"}"#),
        )
        .await;
    assert_eq!(charset.status, 201);

    let calls: Vec<_> = (0..8)
        .map(|index| {
            let host = std::sync::Arc::clone(&host);
            tokio::spawn(async move {
                host.send_as(
                    &writer(),
                    TestRequest::post("/api/admin/genres")
                        .json(&json!({ "Name": format!("Concurrent {index}") })),
                )
                .await
                .json()["Id"]
                    .as_i64()
                    .unwrap()
            })
        })
        .collect();

    let mut ids = Vec::new();
    for call in calls {
        ids.push(call.await.unwrap());
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 8);
}
