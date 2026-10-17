//! Background work the host starts and stops, ported from `IHostedService`.
//!
//! The outbox dispatcher, each module's work queue and the integrity job are
//! hosted services. A module registers them in
//! [`Module::register_services`](crate::module::Module::register_services); the
//! host starts every one after the application is built and cancels them all on
//! shutdown.

use std::future::Future;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;

/// A boxed future, as hosted services and event handlers return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A long-running task with a name.
pub trait HostedService: Send + Sync {
    /// What to call this service in a log record, such as `OrdersOutboxDispatcher`.
    fn name(&self) -> String;

    /// Runs until `stopping` is cancelled.
    ///
    /// Port of `BackgroundService.ExecuteAsync`. Returning early is allowed —
    /// a dispatcher whose `Outbox:Enabled` is false returns at once.
    fn execute(&self, stopping: CancellationToken) -> BoxFuture<'static, ()>;
}

/// The services a host has started, so it can stop them.
#[derive(Debug, Default)]
pub struct RunningServices {
    stopping: CancellationToken,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl RunningServices {
    /// Starts every service on the current runtime.
    #[must_use]
    pub fn start(services: &[std::sync::Arc<dyn HostedService>]) -> Self {
        let stopping = CancellationToken::new();
        let tasks = services
            .iter()
            .map(|service| {
                tracing::debug!(service = service.name(), "starting hosted service");
                tokio::spawn(service.execute(stopping.child_token()))
            })
            .collect();

        Self { stopping, tasks }
    }

    /// Cancels every service and waits for each to return.
    pub async fn stop(mut self) {
        self.stopping.cancel();

        for task in std::mem::take(&mut self.tasks) {
            if let Err(error) = task.await {
                tracing::error!(%error, "a hosted service panicked while stopping");
            }
        }
    }
}

impl Drop for RunningServices {
    fn drop(&mut self) {
        // A host dropped without `stop` — a test, typically — still stops its
        // background work rather than leaking it onto the runtime.
        self.stopping.cancel();
    }
}
