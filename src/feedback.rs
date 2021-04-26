use std::{
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use reqwest::Url;
use slog::{error, info, o, trace, Logger};
use tokio::sync::Notify;

use crate::{
    config,
    error::Error,
    report::{FuzzingStatus, Report, TargetStatus},
};

pub trait FeedbackClient {
    fn message(&self, message: &str);
}

pub struct LoggerClient {
    id: String,
    log: Logger,
}

impl LoggerClient {
    pub fn new(id: &str, log: Logger) -> Self {
        Self { id: id.to_string(), log }
    }
}

impl FeedbackClient for LoggerClient {
    fn message(&self, message: &str) {
        info!(self.log, "{}", message; "client" => &self.id);
    }
}

pub struct Feedback {
    map: Arc<SharedFeedbackMap>,
    client: Arc<Box<dyn FeedbackClient + Send + Sync>>,
    updater: Arc<ScheduledUpdater>,
    report: Arc<Report>,
    log: Logger,
}

impl Feedback {
    pub async fn new<'a>(
        config: &'a config::Feedback,
        client: Box<dyn FeedbackClient + Send + Sync>,
        reports_dir: impl AsRef<Path>,
        reports_url: &'a Option<Url>,
        reports_loc: impl AsRef<Path>,
        log: Logger,
    ) -> Result<Self, Error> {
        let client = Arc::new(client);
        let updater = ScheduledUpdater::new(
            Duration::from_secs(config.start_timeout),
            Duration::from_secs(config.update_timeout),
            Duration::from_secs(config.no_update_timeout),
            log.new(o!("role" => "updater")),
        );
        let report = Report::new(reports_dir.as_ref(), reports_url, reports_loc.as_ref(), log.new(o!("role" => "report"))).await?;
        Ok(Self {
            map: Arc::new(SharedFeedbackMap::new()),
            client,
            updater: Arc::new(updater),
            report: Arc::new(report),
            log,
        })
    }

    pub fn set_total(&self, target: &str, total: u32) {
        self.map.set_total(target, total);
        self.updater.update();
    }

    pub fn add_covered(&self, target: &str, covered: u32) {
        self.map.add_covered(target, covered);
        self.updater.update();
    }

    pub fn add_errors(&self, target: &str, errors: u32) {
        self.map.add_errors(target, errors);
        self.updater.update();
    }

    fn update_text(time: &DateTime<Utc>) -> String {
            let dur = Utc::now().signed_duration_since(time.clone());
            format!(
                "Last coverage update at {}, {}s ago",
                time.format("%Y-%m-%d %H:%M:%S").to_string(),
                dur.num_seconds(),
            )
    }

    pub fn started(&self) {
        self.client.message("Fuzzing is started");
        let client = self.client.clone();
        let report = self.report.clone();
        let map = self.map.clone();
        let log = self.log.clone();
        self.updater.start(move |time, update| {
            if !update {
                client.message(
                    &format!("No coverage updates since {}",
                             time.format("%Y-%m-%d %H:%M:%S").to_string(),
                    )
                );
                return;
            }
            let mut message = Self::update_text(time);
            let snap = map.snapshot();
            let report = report.clone();
            let client = client.clone();
            let log = log.clone();
            tokio::spawn(async move {
                match report.update(&snap).await {
                    Ok(summary) => {
                        message = format!("{}\n{}", message, summary);
                    },
                    Err(e) => {
                        error!(log, "Error updating progress report: {}", e)
                    }
                }
                client.message(&message);
            });
        });
    }

    pub fn stopped(&self) {
        self.client.message("Fuzzing is stopped");
        self.updater.stop();
    }

    pub fn message(&self, msg: impl AsRef<str>) {
        self.client.message(msg.as_ref());
    }
}

pub struct SharedFeedbackMap {
    map: RwLock<FuzzingStatus>,
}

impl SharedFeedbackMap {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(FuzzingStatus::new()),
        }
    }

    pub fn snapshot(&self) -> FuzzingStatus {
        self.map.read().unwrap().clone()
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

struct ScheduledUpdater {
    start_timeout: Duration,
    update_timeout: Duration,
    no_update_timeout: Duration,
    updated: Arc<Notify>,
    stopped: Arc<Notify>,
    log: Logger,
}

impl ScheduledUpdater {
    fn new(start_timeout: Duration, update_timeout: Duration, no_update_timeout: Duration, log: Logger) -> Self {
        Self {
            start_timeout,
            update_timeout,
            no_update_timeout,
            updated: Arc::new(Notify::new()),
            stopped: Arc::new(Notify::new()),
            log,
        }
    }

    fn start<F: Fn(&DateTime<Utc>, bool) + Send + Sync + 'static>(&self, f: F) {
        let start_timeout = self.start_timeout;
        let update_timeout = self.update_timeout;
        let no_update_timeout = self.no_update_timeout;
        let updated = self.updated.clone();
        let stopped = self.stopped.clone();
        let log = self.log.new(o!());
        let mut last_update = Utc::now();
        tokio::spawn(async move {
            let mut timeout = no_update_timeout;
            let mut update = false;
            let mut start = true;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(timeout) => {
                        trace!(log, "Reporting");
                        f(&last_update, update);
                        update = false;
                        timeout = no_update_timeout;
                        start = false;
                    }
                    _ = updated.notified() => {
                        trace!(log, "New update, still waiting");
                        last_update = Utc::now();
                        update = true;
                        timeout = if start { start_timeout } else { update_timeout };
                    }
                    _ = stopped.notified() => {
                        trace!(log, "Requested to stop");
                        return;
                    }
                }
            }
        });
    }

    fn stop(&self) {
        self.stopped.notify_one();
    }

    fn update(&self) {
        self.updated.notify_one();
    }
}
