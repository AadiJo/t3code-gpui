//! The cache write policy (`cachePersistence.ts`): the first save 500 ms after the first change,
//! then at most one save every 10 s, always of the newest value. Dropping the persister flushes
//! a value that was offered but not yet saved.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

const FIRST_SAVE_DELAY: Duration = Duration::from_millis(500);
const SAVE_INTERVAL: Duration = Duration::from_secs(10);

struct Shared<T> {
    pending: Mutex<Option<T>>,
    changed: Notify,
    save: Box<dyn Fn(T) + Send + Sync>,
}

impl<T> Shared<T> {
    fn take(&self) -> Option<T> {
        self.pending.lock().expect("persister lock").take()
    }
}

/// Saves the newest offered value on the schedule above. `save` runs on a blocking thread.
pub struct Persister<T: Send + 'static> {
    shared: Arc<Shared<T>>,
    task: tokio::task::JoinHandle<()>,
}

impl<T: Send + 'static> Persister<T> {
    pub fn spawn(save: impl Fn(T) + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared { pending: Mutex::new(None), changed: Notify::new(), save: Box::new(save) });
        let worker = Arc::clone(&shared);
        let task = tokio::spawn(async move {
            worker.changed.notified().await;
            tokio::time::sleep(FIRST_SAVE_DELAY).await;
            loop {
                if let Some(value) = worker.take() {
                    let saver = Arc::clone(&worker);
                    _ = tokio::task::spawn_blocking(move || (saver.save)(value)).await;
                }
                tokio::time::sleep(SAVE_INTERVAL).await;
                if worker.pending.lock().expect("persister lock").is_none() {
                    worker.changed.notified().await;
                }
            }
        });
        Self { shared, task }
    }

    /// Replaces the value waiting to be saved.
    pub fn offer(&self, value: T) {
        *self.shared.pending.lock().expect("persister lock") = Some(value);
        self.shared.changed.notify_one();
    }

    /// Drops the value waiting to be saved, so a newer decision (such as a removal) wins.
    pub fn cancel(&self) {
        self.shared.take();
    }

    /// Saves the waiting value now, on this thread.
    pub fn flush(&self) {
        if let Some(value) = self.shared.take() {
            (self.shared.save)(value);
        }
    }
}

impl<T: Send + 'static> Drop for Persister<T> {
    fn drop(&mut self) {
        self.task.abort();
        self.flush();
    }
}
