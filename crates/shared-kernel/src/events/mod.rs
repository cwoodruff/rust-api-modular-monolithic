//! Integration events, the outbox and the inbox, ported from
//! `SharedKernel.Events` (ADR-0008).
//!
//! - **Publish with the write.** [`EventPublisher::publish`] adds a row to the
//!   publishing module's outbox on the caller's transaction, so the event
//!   commits with the business change or not at all. There is no in-memory
//!   publish path; that is the point.
//! - **Deliver at least once.** An [`OutboxDispatcher`] polls its module's
//!   outbox and hands each event to every handler subscribed to its type.
//! - **Apply once.** Each handler runs inside [`inbox::run`] on its own
//!   module's database: the `(EventId, HandlerName)` inbox row commits with the
//!   handler's writes, so a redelivered event is skipped.
//! - **Retry, then ask a person.** 1 s, 5 s, 30 s, 2 min, 10 min; the sixth
//!   failure dead-letters the row.
//! - **Order only within one outbox**, by `(OccurredAt, Id)`, and only until a
//!   row fails.

pub mod dispatcher;
pub mod inbox;
pub mod publisher;

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::hosting::BoxFuture;
use crate::persistence::{DbConnection, ModuleDatabase};

pub use dispatcher::{BATCH_SIZE, OutboxDispatcher, OutboxOptions, RETRY_DELAYS};
pub use publisher::EventPublisher;

/// What a handler's failure carries back to the dispatcher.
pub type HandlerError = Box<dyn std::error::Error + Send + Sync>;

/// An event one module publishes and others consume. Port of `IIntegrationEvent`.
///
/// Event types live in the publisher's contracts crate. Serialized with
/// camelCase members, as upstream's web-default JSON options write them.
pub trait IntegrationEvent: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The persisted type name, upstream's `Type.FullName`:
    /// `Orders.Contracts.Events.InvoiceFinalized`. Kept identical so a C# and a
    /// Rust host could read each other's outbox.
    const EVENT_TYPE: &'static str;

    /// The event's identity, which is also its outbox row's key.
    fn event_id(&self) -> Uuid;

    /// When it happened.
    fn occurred_at(&self) -> DateTime<Utc>;
}

/// Reacts to one event type. Port of `IIntegrationEventHandler<TEvent>`.
///
/// The handler writes through `connection`, which is the inbox guard's open
/// transaction on the handler's own module database, and must not commit it:
/// the guard commits the handler's writes and the inbox row together.
pub trait IntegrationEventHandler<E: IntegrationEvent>: Send + Sync + 'static {
    /// The inbox key, upstream's handler `Type.FullName`, such as
    /// `Catalog.Modules.Events.InvoiceFinalizedHandler`.
    const HANDLER_NAME: &'static str;

    /// Applies the event.
    fn handle<'a>(
        &'a self,
        event: &'a E,
        connection: &'a mut DbConnection,
    ) -> BoxFuture<'a, Result<(), HandlerError>>;
}

type ErasedHandle = dyn for<'a> Fn(
        &'a (dyn Any + Send + Sync),
        &'a mut DbConnection,
    ) -> BoxFuture<'a, Result<(), HandlerError>>
    + Send
    + Sync;

/// One handler, subscribed under its module's key. Port of
/// `IntegrationEventSubscription`.
#[derive(Clone)]
pub struct Subscription {
    /// The event type it handles.
    pub event_type: &'static str,
    /// The module whose database the handler writes to.
    pub module_key: &'static str,
    /// The inbox key.
    pub handler_name: &'static str,
    handle: Arc<ErasedHandle>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Subscription")
            .field("event_type", &self.event_type)
            .field("module_key", &self.module_key)
            .field("handler_name", &self.handler_name)
            .finish()
    }
}

impl Subscription {
    /// Subscribes `handler` to `E` under `module_key`.
    ///
    /// Port of `AddIntegrationEventHandler<TEvent, THandler>(moduleKey)`. The
    /// event reaches the handler already decoded, through a typed function
    /// rather than reflection — the same choice upstream made for AOT.
    pub fn new<E, H>(module_key: &'static str, handler: H) -> Self
    where
        E: IntegrationEvent,
        H: IntegrationEventHandler<E>,
    {
        let handler = Arc::new(handler);

        Self {
            event_type: E::EVENT_TYPE,
            module_key,
            handler_name: H::HANDLER_NAME,
            handle: Arc::new(move |event, connection| {
                let handler = Arc::clone(&handler);
                Box::pin(async move {
                    let event = event.downcast_ref::<E>().ok_or_else(|| -> HandlerError {
                        format!("event is not a {}", E::EVENT_TYPE).into()
                    })?;
                    handler.handle(event, connection).await
                })
            }),
        }
    }

    /// Runs the handler.
    ///
    /// # Errors
    ///
    /// Whatever the handler reports.
    pub async fn handle(
        &self,
        event: &(dyn Any + Send + Sync),
        connection: &mut DbConnection,
    ) -> Result<(), HandlerError> {
        (self.handle)(event, connection).await
    }
}

/// Every module's subscriptions, shared with every dispatcher.
///
/// Modules register in the host's order, so a dispatcher built by Orders sees
/// the handlers Catalog and Administration registered before it, and any
/// registered after it: it reads the list per batch, not once.
#[derive(Debug, Clone, Default)]
pub struct EventSubscriptions(Arc<RwLock<Vec<Subscription>>>);

impl EventSubscriptions {
    /// Adds a subscription.
    pub fn add(&self, subscription: Subscription) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(subscription);
    }

    /// The subscriptions to `event_type`, in registration order.
    #[must_use]
    pub fn for_event(&self, event_type: &str) -> Vec<Subscription> {
        self.all()
            .into_iter()
            .filter(|subscription| subscription.event_type == event_type)
            .collect()
    }

    /// Every subscription, in registration order.
    #[must_use]
    pub fn all(&self) -> Vec<Subscription> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The subscriptions one module registered — what resolving a handler
    /// "keyed" by that module yields.
    #[must_use]
    pub fn for_module(&self, module_key: &str) -> Vec<Subscription> {
        self.all()
            .into_iter()
            .filter(|subscription| subscription.module_key == module_key)
            .collect()
    }
}

/// Every module's database, by module name.
///
/// Port of the keyed `DbContext` registrations: the host migrates through it,
/// and a dispatcher finds a handler's inbox through it.
#[derive(Debug, Clone, Default)]
pub struct ModuleDatabases(Arc<RwLock<BTreeMap<String, ModuleDatabase>>>);

impl ModuleDatabases {
    /// Registers a module's database.
    pub fn add(&self, database: ModuleDatabase) {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(database.module().to_owned(), database);
    }

    /// The database a module registered.
    #[must_use]
    pub fn get(&self, module: &str) -> Option<ModuleDatabase> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(module)
            .cloned()
    }

    /// Every registered database.
    #[must_use]
    pub fn all(&self) -> Vec<ModuleDatabase> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}

/// Decodes one event type's payload, for a dispatcher that publishes it.
#[derive(Clone, Copy)]
pub struct EventDecoder {
    /// The type it decodes.
    pub event_type: &'static str,
    decode: fn(&str) -> Result<Arc<dyn Any + Send + Sync>, String>,
}

impl std::fmt::Debug for EventDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventDecoder")
            .field("event_type", &self.event_type)
            .finish()
    }
}

impl EventDecoder {
    /// The decoder for `E`.
    #[must_use]
    pub fn of<E: IntegrationEvent>() -> Self {
        fn decode<E: IntegrationEvent>(
            payload: &str,
        ) -> Result<Arc<dyn Any + Send + Sync>, String> {
            serde_json::from_str::<Option<E>>(payload)
                .map_err(|error| error.to_string())?
                .map(|event| Arc::new(event) as Arc<dyn Any + Send + Sync>)
                .ok_or_else(|| "Payload deserialized to null.".to_owned())
        }

        Self {
            event_type: E::EVENT_TYPE,
            decode: decode::<E>,
        }
    }

    /// Decodes a payload.
    ///
    /// # Errors
    ///
    /// The parser's message, or upstream's `Payload deserialized to null.`
    pub fn decode(&self, payload: &str) -> Result<Arc<dyn Any + Send + Sync>, String> {
        (self.decode)(payload)
    }
}
