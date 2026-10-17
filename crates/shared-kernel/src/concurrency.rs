//! Bounded background work and a cap on expensive operations, ported from
//! `SharedKernel.Concurrency` (ADR-0013).
//!
//! - [`ModuleWorkQueue`]: a bounded queue with one consumer. Producers wait
//!   when it is full, so a backlog applies backpressure instead of growing
//!   memory. The Orders outbox dispatcher and the Reporting integrity job run
//!   their work through one each.
//! - [`ModuleGate`]: a semaphore. Reporting's two view queries hold a slot each,
//!   so at most `MaxConcurrentExpensive` run at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::FutureExt;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::hosting::{BoxFuture, HostedService};

/// The queue capacity when `Concurrency:<Module>:QueueCapacity` is unset.
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;

/// The gate size when `Concurrency:<Module>:MaxConcurrentExpensive` is unset.
pub const DEFAULT_MAX_CONCURRENT_EXPENSIVE: usize = 4;

type WorkItem = Box<dyn FnOnce(CancellationToken) -> BoxFuture<'static, ()> + Send>;

/// Why queued work produced no result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkError {
    /// The queue's consumer has stopped, so nothing will run the item.
    #[error("the {0} work queue has stopped")]
    Stopped(&'static str),

    /// The work panicked. The consumer survives it and moves on.
    #[error("work item failed in the {0} work queue")]
    Panicked(&'static str),
}

/// A bounded queue with one consumer. Port of `ModuleWorkQueue`.
///
/// It is also a [`HostedService`]: the consumer is the hosted task, so the
/// host must start it before anything is queued. A queue whose consumer never
/// starts makes every producer wait, which is the same as upstream.
pub struct ModuleWorkQueue {
    module: &'static str,
    capacity: usize,
    sender: mpsc::Sender<WorkItem>,
    receiver: Mutex<Option<mpsc::Receiver<WorkItem>>>,
    waiting: Arc<AtomicUsize>,
}

impl std::fmt::Debug for ModuleWorkQueue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleWorkQueue")
            .field("module", &self.module)
            .field("capacity", &self.capacity)
            .field("count", &self.count())
            .finish()
    }
}

impl ModuleWorkQueue {
    /// A queue holding at most `capacity` waiting items.
    ///
    /// # Panics
    ///
    /// When `capacity` is zero, as upstream's `ThrowIfNegativeOrZero` refuses it.
    #[must_use]
    pub fn new(module: &'static str, capacity: usize) -> Self {
        assert!(capacity > 0, "a work queue needs a capacity above zero");

        let (sender, receiver) = mpsc::channel(capacity);

        Self {
            module,
            capacity,
            sender,
            receiver: Mutex::new(Some(receiver)),
            waiting: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The module the queue belongs to.
    #[must_use]
    pub fn module(&self) -> &'static str {
        self.module
    }

    /// The most items that can wait.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Items waiting, not counting the one running.
    #[must_use]
    pub fn count(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    /// A shared handle on the waiting count, for the queue-depth gauge.
    #[must_use]
    pub fn depth_probe(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.waiting)
    }

    /// Queues `work`, waiting while the queue is full.
    ///
    /// # Errors
    ///
    /// [`WorkError::Stopped`] when the consumer has gone.
    pub async fn enqueue<F, Fut>(&self, work: F) -> Result<(), WorkError>
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let item: WorkItem = Box::new(move |token| Box::pin(work(token)));

        // Reserve first and count second, so the consumer can never take an
        // item before it has been counted.
        let permit = self
            .sender
            .reserve()
            .await
            .map_err(|_| WorkError::Stopped(self.module))?;

        self.waiting.fetch_add(1, Ordering::SeqCst);
        permit.send(item);

        Ok(())
    }

    /// Queues `work` and waits for its result.
    ///
    /// Port of `RunAsync<T>`. The work receives the queue's stopping token,
    /// not the caller's (F20): a caller that gives up stops waiting, but an
    /// item already queued still runs.
    ///
    /// # Errors
    ///
    /// [`WorkError::Stopped`] when the consumer has gone or stops first;
    /// [`WorkError::Panicked`] when the work panics.
    pub async fn run<T, F, Fut>(&self, work: F) -> Result<T, WorkError>
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        let module = self.module;

        self.enqueue(move |token| async move {
            let outcome = std::panic::AssertUnwindSafe(work(token))
                .catch_unwind()
                .await
                .map_err(|_| WorkError::Panicked(module));
            let _ = reply.send(outcome);
        })
        .await?;

        result.await.map_err(|_| WorkError::Stopped(module))?
    }
}

impl HostedService for Arc<ModuleWorkQueue> {
    fn name(&self) -> String {
        format!("{}WorkQueue", self.module)
    }

    fn execute(&self, stopping: CancellationToken) -> BoxFuture<'static, ()> {
        let queue = Arc::clone(self);

        Box::pin(async move {
            let Some(mut receiver) = queue.receiver.lock().await.take() else {
                tracing::warn!(module = queue.module, "work queue consumer started twice");
                return;
            };

            loop {
                let item = tokio::select! {
                    () = stopping.cancelled() => return,
                    item = receiver.recv() => item,
                };

                let Some(item) = item else { return };
                queue.waiting.fetch_sub(1, Ordering::SeqCst);

                if std::panic::AssertUnwindSafe(item(stopping.clone()))
                    .catch_unwind()
                    .await
                    .is_err()
                {
                    tracing::error!(
                        module = queue.module,
                        event_id = 3001,
                        "Work item failed in the {} work queue",
                        queue.module
                    );
                }
            }
        })
    }
}

/// A cap on concurrent expensive operations. Port of `ModuleGate`.
#[derive(Debug, Clone)]
pub struct ModuleGate {
    module: &'static str,
    max_concurrency: usize,
    semaphore: Arc<Semaphore>,
}

impl ModuleGate {
    /// A gate admitting `max_concurrency` at once.
    ///
    /// # Panics
    ///
    /// When `max_concurrency` is zero.
    #[must_use]
    pub fn new(module: &'static str, max_concurrency: usize) -> Self {
        assert!(max_concurrency > 0, "a gate needs room for at least one");

        Self {
            module,
            max_concurrency,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    /// The module the gate belongs to.
    #[must_use]
    pub fn module(&self) -> &'static str {
        self.module
    }

    /// How many may hold a slot at once.
    #[must_use]
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Slots free right now.
    #[must_use]
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Waits for a slot. Dropping the permit releases it, once.
    pub async fn enter(&self) -> OwnedSemaphorePermit {
        // The semaphore is never closed, so acquiring cannot fail.
        #[allow(clippy::expect_used)]
        Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .expect("the gate's semaphore is never closed")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn the_queue_runs_one_item_at_a_time_and_returns_its_result() {
        let queue = Arc::new(ModuleWorkQueue::new("Test", 4));
        let stopping = CancellationToken::new();
        let consumer = tokio::spawn(HostedService::execute(&queue, stopping.clone()));

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut calls = Vec::new();
        for value in 0..4 {
            let queue = Arc::clone(&queue);
            let running = Arc::clone(&running);
            let peak = Arc::clone(&peak);
            calls.push(tokio::spawn(async move {
                queue
                    .run(move |_| async move {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        running.fetch_sub(1, Ordering::SeqCst);
                        value * 2
                    })
                    .await
            }));
        }

        let mut results = Vec::new();
        for call in calls {
            results.push(call.await.unwrap().unwrap());
        }
        results.sort_unstable();

        assert_eq!(results, [0, 2, 4, 6]);
        assert_eq!(peak.load(Ordering::SeqCst), 1);

        stopping.cancel();
        consumer.await.unwrap();
    }

    #[tokio::test]
    async fn a_producer_waits_while_the_queue_is_full() {
        // No consumer: the first item fills the channel and the second waits.
        let queue = ModuleWorkQueue::new("Test", 1);
        queue.enqueue(|_| async {}).await.unwrap();

        let second = tokio::time::timeout(Duration::from_millis(50), queue.enqueue(|_| async {}));

        assert!(
            second.await.is_err(),
            "the producer should still be waiting"
        );
        assert_eq!(queue.count(), 1, "a producer still waiting is not an item");
    }

    #[tokio::test]
    async fn a_panicking_item_reaches_its_caller_and_the_consumer_survives() {
        let queue = Arc::new(ModuleWorkQueue::new("Test", 2));
        let stopping = CancellationToken::new();
        let consumer = tokio::spawn(HostedService::execute(&queue, stopping.clone()));

        let failed = queue.run(|_| async { panic!("boom") }).await;
        let after: Result<i32, _> = queue.run(|_| async { 7 }).await;

        assert_eq!(failed, Err::<(), _>(WorkError::Panicked("Test")));
        assert_eq!(after, Ok(7));

        stopping.cancel();
        consumer.await.unwrap();
    }

    #[tokio::test]
    async fn the_gate_admits_at_most_its_limit() {
        let gate = ModuleGate::new("Reporting", 2);

        let first = gate.enter().await;
        let _second = gate.enter().await;
        assert_eq!(gate.available(), 0);

        let third = tokio::time::timeout(Duration::from_millis(30), gate.enter()).await;
        assert!(third.is_err(), "a third caller should wait");

        drop(first);
        assert_eq!(gate.available(), 1);
    }
}
