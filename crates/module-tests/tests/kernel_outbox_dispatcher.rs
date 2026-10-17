//! The outbox dispatcher, on the kernel alone. Port of upstream's
//! `Kernel/OutboxDispatcherTests`: a publisher schema and two consumer schemas,
//! raw DDL, no module.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, TimeDelta, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared_kernel::clock::{Clock, ManualClock};
use shared_kernel::concurrency::ModuleWorkQueue;
use shared_kernel::diagnostics::ModuleMeter;
use shared_kernel::events::dispatcher::DispatcherParts;
use shared_kernel::events::{
    EventPublisher, EventSubscriptions, HandlerError, IntegrationEvent, IntegrationEventHandler,
    ModuleDatabases, OutboxDispatcher, OutboxOptions, Subscription,
};
use shared_kernel::hosting::BoxFuture;
use shared_kernel::persistence::{DbConnection, ModuleDatabase};
use shared_kernel::{AppConfig, Environment};
use test_support::postgres;

static NO_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("tests/empty_migrations");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SomethingHappened {
    event_id: uuid::Uuid,
    occurred_at: DateTime<Utc>,
    value: i32,
}

impl IntegrationEvent for SomethingHappened {
    const EVENT_TYPE: &'static str = "Tests.Events.SomethingHappened";

    fn event_id(&self) -> uuid::Uuid {
        self.event_id
    }

    fn occurred_at(&self) -> DateTime<Utc> {
        self.occurred_at
    }
}

/// Records each event in its own schema's `Applied` table, or fails on demand.
struct Recorder {
    schema: &'static str,
    fail: Arc<AtomicBool>,
}

struct One;
struct Two;

trait Named: Send + Sync + 'static {
    const NAME: &'static str;
}
impl Named for One {
    const NAME: &'static str = "Tests.One.Handler";
}
impl Named for Two {
    const NAME: &'static str = "Tests.Two.Handler";
}

struct Handler<N: Named>(Recorder, std::marker::PhantomData<N>);

impl<N: Named> IntegrationEventHandler<SomethingHappened> for Handler<N> {
    const HANDLER_NAME: &'static str = N::NAME;

    fn handle<'a>(
        &'a self,
        event: &'a SomethingHappened,
        connection: &'a mut DbConnection,
    ) -> BoxFuture<'a, Result<(), HandlerError>> {
        Box::pin(async move {
            if self.0.fail.load(Ordering::SeqCst) {
                return Err("the handler was told to fail".into());
            }
            sqlx::query(&format!(
                r#"INSERT INTO "{}"."Applied" ("EventId", "Value") VALUES ($1, $2)"#,
                self.0.schema
            ))
            .bind(event.event_id)
            .bind(event.value)
            .execute(&mut *connection)
            .await?;
            Ok(())
        })
    }
}

struct Fixture {
    database: String,
    publisher_db: ModuleDatabase,
    publisher: EventPublisher,
    dispatcher: OutboxDispatcher,
    clock: ManualClock,
    fail_two: Arc<AtomicBool>,
    meter: Arc<ModuleMeter>,
}

async fn fixture() -> Fixture {
    let server = postgres::server();
    let database = server.empty().await;

    let mut connection = server.connect(&database).await;
    sqlx::raw_sql(
        r#"CREATE SCHEMA kpub; CREATE SCHEMA kone; CREATE SCHEMA ktwo;
           CREATE TABLE kpub."OutboxMessage" (
             "Id" uuid NOT NULL, "EventType" varchar(300) NOT NULL, "Payload" jsonb NOT NULL,
             "OccurredAt" timestamptz NOT NULL, "ProcessedAt" timestamptz NULL, "Attempts" integer NOT NULL,
             "LastError" varchar(2000) NULL, "DeadLetteredAt" timestamptz NULL, "NextAttemptAt" timestamptz NOT NULL,
             CONSTRAINT "PK_OutboxMessage" PRIMARY KEY ("Id"));
           CREATE TABLE kone."InboxMessage" ("EventId" uuid NOT NULL, "HandlerName" varchar(300) NOT NULL,
             "ProcessedAt" timestamptz NOT NULL, PRIMARY KEY ("EventId", "HandlerName"));
           CREATE TABLE ktwo."InboxMessage" ("EventId" uuid NOT NULL, "HandlerName" varchar(300) NOT NULL,
             "ProcessedAt" timestamptz NOT NULL, PRIMARY KEY ("EventId", "HandlerName"));
           CREATE TABLE kone."Applied" ("EventId" uuid NOT NULL, "Value" integer NOT NULL);
           CREATE TABLE ktwo."Applied" ("EventId" uuid NOT NULL, "Value" integer NOT NULL);"#,
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let config = AppConfig::from_values(
        json!({ "ConnectionStrings": { "AppDatabase": server.connection_string(&database) }, "Database": { "MaxPoolSize": 4 } }),
        Environment::Development,
    );
    let publisher_db =
        ModuleDatabase::connect(&config, "Publisher", "kpub", &NO_MIGRATIONS).unwrap();
    let one_db = ModuleDatabase::connect(&config, "One", "kone", &NO_MIGRATIONS).unwrap();
    let two_db = ModuleDatabase::connect(&config, "Two", "ktwo", &NO_MIGRATIONS).unwrap();

    let databases = ModuleDatabases::default();
    databases.add(one_db);
    databases.add(two_db);

    let fail_two = Arc::new(AtomicBool::new(false));
    let subscriptions = EventSubscriptions::default();
    subscriptions.add(Subscription::new::<SomethingHappened, _>(
        "One",
        Handler::<One>(
            Recorder {
                schema: "kone",
                fail: Arc::new(AtomicBool::new(false)),
            },
            std::marker::PhantomData,
        ),
    ));
    subscriptions.add(Subscription::new::<SomethingHappened, _>(
        "Two",
        Handler::<Two>(
            Recorder {
                schema: "ktwo",
                fail: Arc::clone(&fail_two),
            },
            std::marker::PhantomData,
        ),
    ));

    // Whole seconds, so what PostgreSQL stores compares exactly.
    let clock = ManualClock::new(Utc::now().with_nanosecond(0).unwrap());
    let meter = Arc::new(ModuleMeter::new("Publisher"));
    let publisher = EventPublisher::new("kpub", Arc::new(clock.clone()), Arc::clone(&meter));

    let dispatcher = OutboxDispatcher::new(
        "TestOutboxDispatcher",
        DispatcherParts {
            database: publisher_db.clone(),
            subscriptions,
            databases,
            queue: Arc::new(ModuleWorkQueue::new("Publisher", 4)),
            meter: Arc::clone(&meter),
            clock: Arc::new(clock.clone()),
            options: OutboxOptions {
                enabled: false,
                poll_interval: std::time::Duration::from_secs(1),
            },
        },
        &[OutboxDispatcher::publishes::<SomethingHappened>()],
    );

    Fixture {
        database,
        publisher_db,
        publisher,
        dispatcher,
        clock,
        fail_two,
        meter,
    }
}

impl Fixture {
    async fn publish(&self, value: i32) -> uuid::Uuid {
        let event = SomethingHappened {
            event_id: uuid::Uuid::new_v4(),
            occurred_at: self.clock.now(),
            value,
        };
        let mut transaction = self.publisher_db.pool().begin().await.unwrap();
        self.publisher
            .publish(&event, &mut transaction)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        event.event_id
    }

    async fn scalar<T>(&self, sql: &str) -> T
    where
        T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
    {
        let mut connection = postgres::server().connect(&self.database).await;
        sqlx::query_scalar(sql)
            .fetch_one(&mut connection)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn an_event_reaches_every_handler_and_is_marked_processed() {
    let fixture = fixture().await;
    fixture.publish(7).await;

    assert_eq!(fixture.dispatcher.process_batch().await.unwrap(), 1);

    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM kone."Applied""#)
            .await,
        1
    );
    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM ktwo."Applied""#)
            .await,
        1
    );
    assert_eq!(
        fixture
            .scalar::<i64>(
                r#"SELECT COUNT(*) FROM kpub."OutboxMessage" WHERE "ProcessedAt" IS NOT NULL"#
            )
            .await,
        1
    );
    assert_eq!(
        fixture.dispatcher.process_batch().await.unwrap(),
        0,
        "a processed row is not picked up again"
    );
    assert_eq!(
        fixture
            .meter
            .total(shared_kernel::diagnostics::OUTBOX_DISPATCHED),
        1
    );
}

#[tokio::test]
async fn a_redelivered_event_takes_effect_once_per_handler() {
    let fixture = fixture().await;
    fixture.publish(1).await;
    fixture.dispatcher.process_batch().await.unwrap();

    // As though the outbox commit had been lost: the row comes due again.
    let mut connection = postgres::server().connect(&fixture.database).await;
    sqlx::query(r#"UPDATE kpub."OutboxMessage" SET "ProcessedAt" = NULL"#)
        .execute(&mut connection)
        .await
        .unwrap();
    assert_eq!(fixture.dispatcher.process_batch().await.unwrap(), 1);

    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM kone."Applied""#)
            .await,
        1
    );
    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM ktwo."Applied""#)
            .await,
        1
    );
}

#[tokio::test]
async fn a_failing_handler_is_retried_on_the_schedule_then_dead_lettered() {
    let fixture = fixture().await;
    fixture.fail_two.store(true, Ordering::SeqCst);
    let start = fixture.clock.now();
    fixture.publish(3).await;

    let mut elapsed = TimeDelta::zero();
    for (failure, delay) in [1, 5, 30, 120, 600].into_iter().enumerate() {
        assert_eq!(
            fixture.dispatcher.process_batch().await.unwrap(),
            1,
            "failure {}",
            failure + 1
        );

        let (attempts, next): (i32, DateTime<Utc>) = {
            let mut connection = postgres::server().connect(&fixture.database).await;
            sqlx::query_as(r#"SELECT "Attempts", "NextAttemptAt" FROM kpub."OutboxMessage""#)
                .fetch_one(&mut connection)
                .await
                .unwrap()
        };
        assert_eq!(attempts, i32::try_from(failure).unwrap() + 1);
        assert_eq!(next, start + elapsed + TimeDelta::seconds(delay));

        // Not due a moment early.
        fixture.clock.advance(TimeDelta::seconds(delay - 1));
        assert_eq!(fixture.dispatcher.process_batch().await.unwrap(), 0);
        fixture.clock.advance(TimeDelta::seconds(1));
        elapsed += TimeDelta::seconds(delay);
    }

    // The sixth failure dead-letters it.
    assert_eq!(fixture.dispatcher.process_batch().await.unwrap(), 1);
    let (attempts, dead, error): (i32, Option<DateTime<Utc>>, String) = {
        let mut connection = postgres::server().connect(&fixture.database).await;
        sqlx::query_as(
            r#"SELECT "Attempts", "DeadLetteredAt", "LastError" FROM kpub."OutboxMessage""#,
        )
        .fetch_one(&mut connection)
        .await
        .unwrap()
    };
    assert_eq!(attempts, 6);
    assert!(dead.is_some());
    assert_eq!(error, "Tests.Two.Handler: the handler was told to fail");

    fixture.clock.advance(TimeDelta::hours(1));
    assert_eq!(
        fixture.dispatcher.process_batch().await.unwrap(),
        0,
        "a dead letter waits for a person"
    );

    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM kone."Applied""#)
            .await,
        1,
        "the healthy handler applied once"
    );
    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM ktwo."Applied""#)
            .await,
        0
    );
    assert_eq!(
        fixture
            .meter
            .total(shared_kernel::diagnostics::OUTBOX_RETRIED),
        5
    );
    assert_eq!(
        fixture
            .meter
            .total(shared_kernel::diagnostics::OUTBOX_DEAD_LETTERED),
        1
    );
}

#[tokio::test]
async fn an_unknown_event_type_is_a_failed_delivery() {
    let fixture = fixture().await;
    let mut connection = postgres::server().connect(&fixture.database).await;
    sqlx::query(
        r#"INSERT INTO kpub."OutboxMessage" ("Id", "EventType", "Payload", "OccurredAt", "Attempts", "NextAttemptAt")
           VALUES ($1, 'Tests.Events.Nope', '{}'::jsonb, $2, 0, $2)"#,
    )
    .bind(uuid::Uuid::new_v4())
    .bind(fixture.clock.now())
    .execute(&mut connection)
    .await
    .unwrap();

    fixture.dispatcher.process_batch().await.unwrap();

    assert_eq!(
        fixture
            .scalar::<String>(r#"SELECT "LastError" FROM kpub."OutboxMessage""#)
            .await,
        "Unknown event type 'Tests.Events.Nope'."
    );
}

#[tokio::test]
async fn a_publish_is_written_only_when_the_callers_transaction_commits() {
    let fixture = fixture().await;
    let event = SomethingHappened {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: fixture.clock.now(),
        value: 1,
    };

    let mut transaction = fixture.publisher_db.pool().begin().await.unwrap();
    fixture
        .publisher
        .publish(&event, &mut transaction)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM kpub."OutboxMessage""#)
            .await,
        0
    );

    fixture.publish(2).await;
    assert_eq!(
        fixture
            .scalar::<i64>(r#"SELECT COUNT(*) FROM kpub."OutboxMessage""#)
            .await,
        1
    );
    let payload: serde_json::Value = fixture
        .scalar::<sqlx::types::Json<serde_json::Value>>(
            r#"SELECT "Payload" FROM kpub."OutboxMessage""#,
        )
        .await
        .0;
    assert_eq!(payload["value"], 2, "payloads are camelCase");
}
