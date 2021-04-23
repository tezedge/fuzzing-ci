use std::{
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use reqwest::Url;
use slog::{debug, error, info, o, trace, Logger};
use tokio::sync::Notify;

use crate::{
    config,
    error::Error,
    report::{FuzzingStatus, Report, TargetStatus},
};

pub trait FeedbackClient {
    fn message(&self, message: String);
}

pub struct LoggerClient {
    id: String,
    log: Logger,
}

impl LoggerClient {
    pub fn new(id: String, log: Logger) -> Self {
        Self { id, log }
    }
}

impl FeedbackClient for LoggerClient {
    fn message(&self, message: String) {
        info!(self.log, "{}", message; "client" => &self.id);
    }
}

pub struct Feedback {
    map: Arc<SharedFeedbackMap>,
    client: Arc<Box<dyn FeedbackClient + Send + Sync>>,
    updater: Arc<ScheduledUpdater>,
    report: Arc<Report>,
    reports_url: Option<Url>,
    log: Logger,
}

impl Feedback {
    pub async fn new(
        config: &config::Feedback,
        client: Box<dyn FeedbackClient + Send + Sync>,
        report_dir: impl AsRef<Path>,
        reports_url: Option<Url>,
        reports_loc: impl AsRef<str>,
        log: Logger,
    ) -> Result<Self, Error> {
        let client = Arc::new(client);
        let updater = ScheduledUpdater::new(
            Duration::from_secs(config.update_timeout),
            Duration::from_secs(config.no_update_timeout),
            log.new(o!("role" => "updater")),
        );
        let reports_dir = report_dir.as_ref().join(reports_loc.as_ref());
        let reports_url =
            reports_url.map_or_else(|| Ok(None), |u| u.join(reports_loc.as_ref()).map(Some))?;
        let report = Report::new(reports_dir, log.new(o!("role" => "report"))).await?;
        Ok(Self {
            map: Arc::new(SharedFeedbackMap::new()),
            client,
            updater: Arc::new(updater),
            report: Arc::new(report),
            reports_url,
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

    pub fn started(&self) {
        self.client.message("Fuzzing is started".to_string());
        let client = self.client.clone();
        let report = self.report.clone();
        let map = self.map.clone();
        let reports_url = self.reports_url.clone();
        let log = self.log.clone();
        self.updater.start(move |time| {
            let dur = Utc::now().signed_duration_since(time.clone());
            let mut message = format!(
                "Last coverage update at {}, {}s ago",
                time.format("%Y-%m-%d %H:%M:%S").to_string(),
                dur.num_seconds(),
            );
            if let Some(url) = &reports_url {
                message = format!("{}\nReport is available at {}", message, url.to_string());
            }
            client.message(message);
            let snap = map.snapshot();
            let report = report.clone();
            let log = log.clone();
            tokio::spawn(async move {
                if let Err(e) = report.update(&snap).await {
                    error!(log, "Error updating progress report: {}", e);
                } else {
                    debug!(log, "Updated progress report");
                }
                report
                    .generate_report(&snap)
                    .await
                    .expect("error generating report");
            });
        });
    }

    pub fn stopped(&self) {
        self.client.message("Fuzzing is stopped".to_string());
        self.updater.stop();
    }

    pub fn message(&self, msg: String) {
        self.client.message(msg);
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
    update_timeout: Duration,
    no_update_timeout: Duration,
    updated: Arc<Notify>,
    stopped: Arc<Notify>,
    log: Logger,
}

impl ScheduledUpdater {
    fn new(update_timeout: Duration, no_update_timeout: Duration, log: Logger) -> Self {
        Self {
            update_timeout,
            no_update_timeout,
            updated: Arc::new(Notify::new()),
            stopped: Arc::new(Notify::new()),
            log,
        }
    }

    fn start<F: Fn(&DateTime<Utc>) + Send + Sync + 'static>(&self, f: F) {
        let update_timeout = self.update_timeout;
        let no_update_timeout = self.no_update_timeout;
        let updated = self.updated.clone();
        let stopped = self.stopped.clone();
        let log = self.log.new(o!());
        let mut last_update = Utc::now();
        tokio::spawn(async move {
            let mut timeout = no_update_timeout;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(timeout) => {
                        trace!(log, "Reporting");
                        f(&last_update);
                        timeout = no_update_timeout;
                    }
                    _ = updated.notified() => {
                        trace!(log, "New update, still waiting");
                        last_update = Utc::now();
                        timeout = update_timeout;
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
