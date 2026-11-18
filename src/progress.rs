// Copyright 2026 Ken An. All Rights Reserved.

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug)]
pub enum Event {
    Log(String),
    Status(String),
    Progress {
        completed: usize,
        total: usize,
        detail: String,
    },
    Output(PathBuf),
}

/// Both frontends use the same packager; GUI work never writes to UI state directly.
#[derive(Clone, Default)]
pub struct Reporter {
    callback: Option<Arc<dyn Fn(Event) + Send + Sync>>,
    pub stop_after_current: Arc<AtomicBool>,
    pub force_stop: Arc<AtomicBool>,
}

impl Reporter {
    pub fn new(callback: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self {
            callback: Some(Arc::new(callback)),
            ..Self::default()
        }
    }

    pub fn log(&self, message: impl Into<String>) {
        let message = message.into();
        if let Some(callback) = &self.callback {
            callback(Event::Log(message));
        } else {
            println!("{message}");
        }
    }

    pub fn status(&self, message: impl Into<String>) {
        let message = message.into();
        self.log(message.clone());
        if let Some(callback) = &self.callback {
            callback(Event::Status(message));
        }
    }

    pub fn progress(&self, completed: usize, total: usize, detail: impl Into<String>) {
        if let Some(callback) = &self.callback {
            callback(Event::Progress {
                completed,
                total,
                detail: detail.into(),
            });
        }
    }

    pub fn output(&self, path: PathBuf) {
        if let Some(callback) = &self.callback {
            callback(Event::Output(path));
        }
    }

    pub fn stopping(&self) -> bool {
        self.stop_after_current.load(Ordering::Relaxed) || self.force_stopping()
    }

    pub fn force_stopping(&self) -> bool {
        self.force_stop.load(Ordering::Relaxed)
    }
}
