//! File-backed async webhook delivery queue with exponential backoff,
//! jitter, and a visible dead-letter file.

use crate::outbound_dispatcher::post_json_with_retry;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Single durable webhook/relay delivery job persisted in the file-backed queue.
#[derive(Debug, Clone)]
pub struct DeliveryJob {
    /// Stable job identifier used in queue and dead-letter records.
    pub id: String,
    /// Destination `http://` URL.
    pub url: String,
    /// Extra `Header: value` pairs sent with the JSON body.
    pub headers: Vec<(String, String)>,
    /// JSON body delivered with `Content-Type: application/json`.
    pub body: String,
    /// Attempts already performed (incremented on each drain pass).
    pub attempts: u32,
}

/// File-backed async webhook delivery queue with exponential backoff and a
/// visible dead-letter file. Pending jobs survive restarts via
/// `<queue_path>`; permanently failed jobs land in `<dead_letter_path>`
/// (JSONL with attempts + last error) instead of vanishing silently.
#[derive(Debug)]
pub struct DeliveryQueue {
    queue_path: String,
    dead_letter_path: String,
    max_attempts: u32,
    pending_jobs: Vec<DeliveryJob>,
    dead_lettered: u64,
    delivered: u64,
}

impl DeliveryQueue {
    /// Creates a queue backed by `queue_path`, spilling to `dead_letter_path`.
    pub fn new(queue_path: &str, dead_letter_path: &str, max_attempts: u32) -> Self {
        let mut queue = Self {
            queue_path: queue_path.to_string(),
            dead_letter_path: dead_letter_path.to_string(),
            max_attempts: max_attempts.max(1),
            pending_jobs: Vec::new(),
            dead_lettered: 0,
            delivered: 0,
        };
        queue.reload();
        queue
    }

    /// Computes the backoff delay in milliseconds for `attempt` (0-based):
    /// `base_ms * 2^attempt` capped at 30s, plus deterministic jitter.
    pub fn backoff_ms(attempt: u32, base_ms: u64) -> u64 {
        let shift = attempt.min(8);
        let grown = base_ms.saturating_mul(1u64 << shift).min(30_000);
        let jitter_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        let jitter = jitter_seed % 51;
        grown.saturating_add(jitter).min(30_050)
    }

    /// Enqueues a delivery job and persists the backlog to disk.
    pub fn enqueue(&mut self, job: DeliveryJob) {
        self.pending_jobs.push(job);
        self.persist_queue();
    }

    /// Number of jobs awaiting delivery.
    pub fn pending(&self) -> usize {
        self.pending_jobs.len()
    }

    /// Number of jobs moved to the dead-letter file in this process.
    pub fn dead_lettered(&self) -> u64 {
        self.dead_lettered
    }

    /// Number of jobs delivered in this process.
    pub fn delivered(&self) -> u64 {
        self.delivered
    }

    /// Performs one drain pass over all pending jobs (one attempt each),
    /// persisting the backlog and spilling exhausted jobs to dead-letter.
    pub fn drain_once(&mut self, timeout_ms: u64) {
        let jobs: Vec<DeliveryJob> = std::mem::take(&mut self.pending_jobs);
        let mut remaining = Vec::new();
        for mut job in jobs {
            job.attempts = job.attempts.saturating_add(1);
            let headers: Vec<(&str, &str)> = job
                .headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let outcome = post_json_with_retry(&job.url, &headers, &job.body, timeout_ms, 0);
            match outcome {
                Ok(status) if (200..300).contains(&status) => {
                    self.delivered = self.delivered.saturating_add(1);
                }
                Ok(status) => {
                    self.record_attempt(&mut remaining, job, format!("upstream {status}"));
                }
                Err(err) => {
                    self.record_attempt(&mut remaining, job, err);
                }
            }
        }
        self.pending_jobs = remaining;
        self.persist_queue();
    }

    /// Records a failed attempt: re-queues or dead-letters with visibility.
    fn record_attempt(
        &mut self,
        remaining: &mut Vec<DeliveryJob>,
        job: DeliveryJob,
        last_error: String,
    ) {
        if job.attempts >= self.max_attempts {
            self.dead_lettered = self.dead_lettered.saturating_add(1);
            let record = format!(
                r#"{{"job_id":"{}","url":"{}","attempts":{},"last_error":"{}"}}"#,
                job.id,
                job.url,
                job.attempts,
                last_error.replace('"', "'")
            );
            eprintln!(
                "{{\"level\":\"ERROR\",\"service\":\"sonaraim-b2b\",\"component\":\"delivery_queue\",\"job_id\":\"{}\",\"error\":\"dead_lettered\"}}",
                job.id
            );
            let _ = Self::append_line(&self.dead_letter_path, &record);
        } else {
            thread::sleep(Duration::from_millis(Self::backoff_ms(
                job.attempts.saturating_sub(1),
                50,
            )));
            remaining.push(job);
        }
    }

    /// Persists the pending backlog atomically (tmp + rename).
    fn persist_queue(&self) {
        let mut out = String::new();
        for job in &self.pending_jobs {
            out.push_str(&format!(
                r#"{{"job_id":"{}","url":"{}","body_len":{},"attempts":{}}}"#,
                job.id,
                job.url,
                job.body.len(),
                job.attempts
            ));
            out.push('\n');
        }
        let tmp = format!("{}.tmp", self.queue_path);
        if std::fs::write(&tmp, &out).is_ok() {
            let _ = std::fs::rename(&tmp, &self.queue_path);
        }
    }

    /// Appends one JSONL record to `path`, creating parent directories.
    fn append_line(path: &str, line: &str) -> std::io::Result<()> {
        use std::io::Write as _;
        if let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{line}")?;
        file.sync_all()?;
        Ok(())
    }

    /// Reloads the pending backlog length from disk (bodies re-fetched by producers).
    fn reload(&mut self) {
        if let Ok(content) = std::fs::read_to_string(&self.queue_path) {
            self.pending_jobs = content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| {
                    Some(DeliveryJob {
                        id: crate::webhooks::json_str(l, "job_id")?,
                        url: crate::webhooks::json_str(l, "url")?,
                        headers: Vec::new(),
                        body: String::new(),
                        attempts: crate::webhooks::json_u64(l, "attempts").unwrap_or(0) as u32,
                    })
                })
                .collect();
        }
    }
}
