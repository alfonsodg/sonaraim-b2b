//! Delivery-queue wiring for the live B2B routes: durable enqueue,
//! exactly-once fast-path confirmation, and the background drain worker.
//!
//! Split out of [`crate::http_api`] to keep that module under the
//! repository's 500-line file limit.

use crate::delivery_queue::DeliveryQueue;
use crate::http_api::B2bState;
use std::sync::{Arc, Mutex};
use std::thread;

/// Returns the background drain interval in milliseconds
/// (`B2B_DELIVERY_DRAIN_MS`, default 250).
pub fn delivery_drain_interval_ms() -> u64 {
    std::env::var("B2B_DELIVERY_DRAIN_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(250)
}

/// Generates a stable unique delivery id (`dlv_<nanos>_<pid>`) returned by
/// the live routes and used as the durable queue job key.
pub fn new_delivery_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("dlv_{nanos}_{}", std::process::id())
}

/// Enables the durable async delivery queue backing the live routes.
///
/// Pending jobs persist in `<queue_path>` and exhausted jobs land in
/// `<dead_letter_path>`; a background worker (spawned by
/// [`spawn_delivery_worker`]) drains the backlog with backoff+jitter.
/// `max_attempts` is clamped to at least 1.
pub fn enable_delivery_queue(
    state: &mut B2bState,
    queue_path: &str,
    dead_letter_path: &str,
    max_attempts: u32,
) {
    let queue = DeliveryQueue::new(queue_path, dead_letter_path, max_attempts);
    state.delivery_queue = Some(Arc::new(Mutex::new(queue)));
}

/// Derives queue/dead-letter sibling paths from the configured
/// `storage_path` (`<storage>.queue.jsonl`, `<storage>.dead-letter.jsonl`)
/// and enables the delivery queue. No-op without a `storage_path`.
pub fn enable_delivery_queue_from_storage(state: &mut B2bState, max_attempts: u32) {
    if let Some(ref storage) = state.storage_path.clone() {
        let queue_path = format!("{storage}.queue.jsonl");
        let dead_letter_path = format!("{storage}.dead-letter.jsonl");
        enable_delivery_queue(state, &queue_path, &dead_letter_path, max_attempts);
    }
}

/// Enqueues a durable delivery job, returning its stable job id.
/// Returns `None` when the delivery queue is not enabled.
pub fn enqueue_delivery(
    state: &B2bState,
    url: &str,
    headers: Vec<(String, String)>,
    body: &str,
) -> Option<String> {
    let shared = state.delivery_queue.as_ref()?;
    let job = crate::delivery_queue::DeliveryJob {
        id: new_delivery_id(),
        url: url.to_string(),
        headers,
        body: body.to_string(),
        attempts: 0,
    };
    let id = job.id.clone();
    if let Ok(mut queue) = shared.lock() {
        queue.enqueue(job);
        Some(id)
    } else {
        None
    }
}

/// Drops a queued job that the synchronous fast path already delivered
/// (exactly-once: the worker never redelivers a fast-path success).
pub fn confirm_delivered(state: &B2bState, delivery_id: &str) {
    if let Some(ref shared) = state.delivery_queue
        && let Ok(mut queue) = shared.lock()
    {
        queue.remove_job(delivery_id);
    }
}

/// Spawns the background delivery-queue drain worker when the queue is
/// enabled: every [`delivery_drain_interval_ms`] it performs one drain pass
/// (backoff+jitter per job, dead-letter on exhaustion). No-op otherwise.
pub fn spawn_delivery_worker(shared: Arc<Mutex<B2bState>>) {
    let enabled = shared
        .lock()
        .map(|guard| guard.delivery_queue.is_some())
        .unwrap_or(false);
    if !enabled {
        return;
    }
    thread::spawn(move || {
        loop {
            thread::sleep(std::time::Duration::from_millis(
                delivery_drain_interval_ms(),
            ));
            let timeout = crate::outbound_dispatcher::configured_timeout_ms();
            if let Ok(guard) = shared.lock()
                && let Some(ref queue) = guard.delivery_queue
                && let Ok(mut queue) = queue.lock()
            {
                queue.drain_once(timeout);
            }
        }
    });
}
