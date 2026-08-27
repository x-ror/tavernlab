//! Work that takes longer than a request should.
//!
//! A tier matrix is quadratic in the size of the field, so it is started by
//! one request and polled by the next — `POST /api/tiers` answers with a job
//! id and `GET /api/job/{id}` reports on it. The front end polls every
//! 700 ms and shows the progress lines as they arrive.
//!
//! Finished jobs are kept so a poll that arrives after the work is done still
//! finds the result, and the oldest are dropped once there are enough of
//! them: this is a local app whose user starts a handful of runs, not a queue.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How many finished jobs to keep. Enough that a slow poll never misses one.
const KEEP: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Running,
    Done,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Done => "done",
            Status::Error => "error",
        }
    }
}

pub struct Job {
    pub status: Status,
    /// Lines written by the work, in order.
    pub progress: Vec<String>,
    /// The finished JSON body, verbatim.
    pub result: Option<String>,
    pub error: Option<String>,
    pub started: Instant,
    pub finished: Option<Duration>,
}

/// Every job this process has run recently.
#[derive(Clone, Default)]
pub struct Jobs {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    jobs: HashMap<String, Job>,
    /// Ids in start order, so the oldest finished job is the one dropped.
    order: Vec<String>,
    /// Jobs started since the process began, for `/api/metrics`.
    pub started_total: u64,
}

/// The handle a running job writes progress through.
pub struct Progress {
    jobs: Jobs,
    id: String,
}

impl Progress {
    pub fn say(&self, line: impl Into<String>) {
        if let Ok(mut inner) = self.jobs.inner.lock()
            && let Some(job) = inner.jobs.get_mut(&self.id)
        {
            job.progress.push(line.into());
        }
    }
}

impl Jobs {
    /// Run `work` on its own thread. Returns the id to poll.
    ///
    /// `work` returns the response body it wants published, or an error
    /// message for the user.
    pub fn start(
        &self,
        work: impl FnOnce(&Progress) -> Result<String, String> + Send + 'static,
    ) -> String {
        let id = {
            let mut inner = self.inner.lock().expect("jobs lock");
            inner.next_id += 1;
            inner.started_total += 1;
            let id = format!("j{}", inner.next_id);
            inner.jobs.insert(
                id.clone(),
                Job {
                    status: Status::Running,
                    progress: Vec::new(),
                    result: None,
                    error: None,
                    started: Instant::now(),
                    finished: None,
                },
            );
            inner.order.push(id.clone());
            let live: Vec<String> = inner
                .order
                .iter()
                .filter(|i| {
                    inner
                        .jobs
                        .get(*i)
                        .is_some_and(|j| j.status == Status::Running)
                })
                .cloned()
                .collect();
            // Trim finished jobs, never running ones.
            while inner.order.len() > KEEP.max(live.len()) {
                let oldest = inner.order.remove(0);
                if inner
                    .jobs
                    .get(&oldest)
                    .is_some_and(|j| j.status == Status::Running)
                {
                    inner.order.push(oldest);
                    break;
                }
                inner.jobs.remove(&oldest);
            }
            id
        };

        let handle = Progress {
            jobs: self.clone(),
            id: id.clone(),
        };
        let jobs = self.clone();
        let done_id = id.clone();
        let spawned = std::thread::Builder::new()
            .name("tavernlab-job".into())
            .spawn(move || {
                let outcome = work(&handle);
                if let Ok(mut inner) = jobs.inner.lock()
                    && let Some(job) = inner.jobs.get_mut(&done_id)
                {
                    job.finished = Some(job.started.elapsed());
                    match outcome {
                        Ok(body) => {
                            job.result = Some(body);
                            job.status = Status::Done;
                        }
                        Err(msg) => {
                            job.error = Some(msg);
                            job.status = Status::Error;
                        }
                    }
                }
            });
        if spawned.is_err()
            && let Ok(mut inner) = self.inner.lock()
            && let Some(job) = inner.jobs.get_mut(&id)
        {
            // A job the OS refused to start must not sit in "running"
            // forever while the UI polls it.
            job.status = Status::Error;
            job.error = Some("could not start a worker thread".into());
        }
        id
    }

    /// Read a job, if it is still known.
    pub fn with<T>(&self, id: &str, f: impl FnOnce(&Job) -> T) -> Option<T> {
        let inner = self.inner.lock().ok()?;
        inner.jobs.get(id).map(f)
    }

    pub fn started_total(&self) -> u64 {
        self.inner.lock().map(|i| i.started_total).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn wait_for(jobs: &Jobs, id: &str) -> Status {
        for _ in 0..2000 {
            if let Some(s) = jobs.with(id, |j| j.status)
                && s != Status::Running
            {
                return s;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("job never finished");
    }

    #[test]
    fn a_finished_job_keeps_its_result_and_progress() {
        let jobs = Jobs::default();
        let id = jobs.start(|p| {
            p.say("half way");
            Ok("{\"ok\":true}".to_string())
        });
        assert_eq!(wait_for(&jobs, &id), Status::Done);
        let (progress, result) = jobs
            .with(&id, |j| (j.progress.clone(), j.result.clone()))
            .unwrap();
        assert_eq!(progress, vec!["half way".to_string()]);
        assert_eq!(result.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn a_failed_job_reports_why() {
        let jobs = Jobs::default();
        let id = jobs.start(|_| Err("нереалізовані карти: Fireball".into()));
        assert_eq!(wait_for(&jobs, &id), Status::Error);
        let err = jobs.with(&id, |j| j.error.clone()).unwrap();
        assert_eq!(err.as_deref(), Some("нереалізовані карти: Fireball"));
    }

    #[test]
    fn an_unknown_id_is_not_a_panic() {
        let jobs = Jobs::default();
        assert!(jobs.with("nope", |_| ()).is_none());
    }

    #[test]
    fn old_jobs_are_dropped_but_running_ones_survive() {
        let jobs = Jobs::default();
        let (tx, rx) = mpsc::channel::<()>();
        let blocked = jobs.start(move |_| {
            let _ = rx.recv();
            Ok("{}".into())
        });
        for _ in 0..(KEEP + 8) {
            let id = jobs.start(|_| Ok("{}".into()));
            wait_for(&jobs, &id);
        }
        assert!(
            jobs.with(&blocked, |j| j.status) == Some(Status::Running),
            "a running job was evicted"
        );
        let _ = tx.send(());
        assert_eq!(wait_for(&jobs, &blocked), Status::Done);
        assert!(jobs.started_total() >= KEEP as u64);
    }
}
