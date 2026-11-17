//! (0.18.1) The long job running now (applying a plan, undoing a batch…).
//! Such a job holds the app lock until it ends; the windows show a bar
//! with what is running and how far it is, and a window opened (or
//! reloaded with F5) meanwhile asks `current_job` so it does not look dead.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::CmdResult;
use crate::state::AppState;

pub const EV_JOB: &str = "tidy://job";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDto {
    /// "미리보기 적용 중"
    pub what: String,
    /// Items done / all items (`total == 0`: not counted).
    pub done: usize,
    pub total: usize,
    /// Unix seconds.
    pub started: i64,
}

/// Ends the job (and tells the windows) when dropped, also on an error.
pub struct Job {
    app: AppHandle,
    last: std::cell::Cell<Instant>,
}

/// Start showing `what`; `total` items (0 = not counted).
pub fn begin(app: &AppHandle, what: impl Into<String>, total: usize) -> Job {
    let dto = JobDto {
        what: what.into(),
        done: 0,
        total,
        started: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };
    set(app, Some(dto));
    Job {
        app: app.clone(),
        last: std::cell::Cell::new(Instant::now()),
    }
}

fn set(app: &AppHandle, job: Option<JobDto>) {
    *app.state::<AppState>()
        .job
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = job.clone();
    let _ = app.emit(EV_JOB, job);
}

impl Job {
    /// Items done so far; the windows hear about it at most 4 times a second.
    pub fn progress(&self, done: usize, total: usize) {
        let state = self.app.state::<AppState>();
        let snapshot = {
            let mut g = state.job.lock().unwrap_or_else(|e| e.into_inner());
            let Some(j) = g.as_mut() else { return };
            j.done = done;
            j.total = total;
            j.clone()
        };
        if self.last.get().elapsed() >= Duration::from_millis(250) || done >= total {
            self.last.set(Instant::now());
            let _ = self.app.emit(EV_JOB, Some(snapshot));
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        set(&self.app, None);
    }
}

/// The job running now, if any (a window just opened or reloaded).
#[tauri::command]
pub async fn current_job(app: AppHandle) -> CmdResult<Option<JobDto>> {
    Ok(app
        .state::<AppState>()
        .job
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone())
}
