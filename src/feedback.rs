use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use slog::{error, info, Logger};

pub trait Feedback {
    fn started(self: &Arc<Self>, description: String);
    fn set_total(self: &Arc<Self>, target: impl AsRef<str>, total: u32);
    fn add_covered(self: &Arc<Self>, target: impl AsRef<str>, covered: u32);
    fn add_errors(self: &Arc<Self>, target: impl AsRef<str>, errors: u32);
    fn stopped(self: &Arc<Self>);
    fn message(self: &Arc<Self>, msg: String);
}

#[derive(Clone, derive_new::new, Default)]
pub struct TargetStatus {
    pub total: u32,
    pub covered: u32,
    pub errors: u32,
}

pub struct SharedFeedbackMap {
    map: RwLock<HashMap<String, TargetStatus>>,
}

impl SharedFeedbackMap {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }

    #[inline]
    pub fn get(&self, target: impl AsRef<str>) -> Option<TargetStatus> {
        self.map.read().unwrap().get(target.as_ref()).cloned()
    }

    pub fn as_table(&self) -> Vec<(String, TargetStatus)> {
        self.map.read().unwrap().clone().into_iter().collect()
    }

    pub fn set_total(&self, target: impl AsRef<str>, total: u32) {
        self.map
            .write()
            .unwrap()
            .insert(target.as_ref().into(), TargetStatus::new(total, 0, 0));
    }

    pub fn add_covered(&self, target: impl AsRef<str>, covered: u32) {
        self.map
            .write()
            .unwrap()
            .get_mut(target.as_ref())
            .map(|s| s.covered += covered);
    }

    pub fn add_errors(&self, target: impl AsRef<str>, errors: u32) {
        self.map
            .write()
            .unwrap()
            .get_mut(target.as_ref())
            .map(|s| s.errors += errors);
    }
}

pub struct LoggerFeedback {
    map: SharedFeedbackMap,
    log: Logger,
}

impl LoggerFeedback {
    pub fn new(log: Logger) -> Self {
        Self {
            map: SharedFeedbackMap::new(),
            log,
        }
    }
    fn feedback(&self, target: impl AsRef<str>) {
        if let Some(status) = self.map.get(target.as_ref()) {
            info!(self.log, "[X] {}", target.as_ref(); "total" => status.total, "covered" => status.covered, "errors" => status.errors);
        } else {
            error!(self.log, "No such target: {}", target.as_ref());
        }
    }
}

impl Feedback for LoggerFeedback {
    fn set_total(self: &Arc<Self>, target: impl AsRef<str>, total: u32) {
        let target = target.as_ref();
        self.map.set_total(target, total);
        self.feedback(target);
    }

    fn add_covered(self: &Arc<Self>, target: impl AsRef<str>, covered: u32) {
        let target = target.as_ref();
        self.map.add_covered(target, covered);
        self.feedback(target);
    }

    fn add_errors(self: &Arc<Self>, target: impl AsRef<str>, errors: u32) {
        let target = target.as_ref();
        self.map.add_errors(target, errors);
        self.feedback(target);
    }

    fn started(self: &Arc<Self>, description: String) {
        info!(self.log, "Fuzzing is started: {}", description);
    }

    fn stopped(self: &Arc<Self>) {}

    fn message(self: &Arc<Self>, msg: String) {
        info!(self.log, "Message: {}", msg);
    }
}
