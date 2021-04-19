use std::{borrow::Cow, collections::HashMap, fmt::Write, io, sync::Arc, time::Duration};

use reqwest::header::AUTHORIZATION;
use slog::{error, o, trace, warn, Logger};
use tokio::sync::Notify;

use crate::{
    config::Slack,
    feedback::{Feedback, SharedFeedbackMap},
};

const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";

pub struct SlackFeedback {
    channel: String,
    token: String,
    map: SharedFeedbackMap,
    updater: ScheduledUpdater,
    log: Logger,
}

#[derive(serde::Deserialize, Debug)]
pub struct JsonResponse {
    ok: bool,
    warning: Option<String>,
    error: Option<String>,
}

impl SlackFeedback {
    pub async fn start(config: Slack, log: Logger) -> io::Result<Self> {
        let meself = SlackFeedback {
            channel: config.channel,
            token: format!("Bearer {}", config.token),
            map: SharedFeedbackMap::new(),
            updater: ScheduledUpdater::new(log.clone()),
            log,
        };
        Ok(meself)
    }

    fn message_json<'a>(&self, text: impl Into<Cow<'a, str>>) -> HashMap<String, String> {
        [
            ("channel", self.channel.clone()),
            ("text", text.into().into_owned()),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
    }

    async fn send_message(&self, text: impl AsRef<str>) -> io::Result<()> {
        trace!(self.log, "Sending to slack"; "text" => text.as_ref());
        let client = reqwest::Client::new();
        let response = client
            .post(POST_MESSAGE_URL)
            .header(AUTHORIZATION, &self.token)
            .json(&self.message_json(text.as_ref()))
            .send()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
            .json::<JsonResponse>()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        trace!(self.log, "Sent to slack"; "response" => format!("{:?}", response));

        if response.ok {
            if let Some(warn) = response.warning {
                if warn != "missing_charset" {
                    warn!(self.log, "Posting message"; "warning" => warn);
                }
            }
            Ok(())
        } else {
            let error = response.error.unwrap_or("unknown error".to_string());
            error!(self.log, "Posting message"; "error" => &error);
            Err(io::Error::new(io::ErrorKind::Other, error))
        }
    }

    fn report(self: &Arc<Self>, description: &String) {
        let mut table = self.map.as_table();
        table.sort_by(|a, b| a.0.cmp(&b.0));
        let mut r = String::new();
        writeln!(r, "{}", description).unwrap();
        for (target, status) in table {
            writeln!(
                r,
                "- *{}*: {}/{} edges, {} errors",
                target, status.covered, status.total, status.errors
            )
            .unwrap();
        }
        let meself = self.clone();
        tokio::spawn(async move {
            if let Err(e) = meself.send_message(r).await {
                error!(meself.log, "Can't send a message to slack"; "error" => e);
            }
        });
    }
}

const DURATION_SHORT: Duration = Duration::from_secs(60);
const DURATION_LONG: Duration = Duration::from_secs(3600);

impl Feedback for SlackFeedback {
    fn set_total(self: &Arc<Self>, target: impl AsRef<str>, total: u32) {
        self.map.set_total(target, total);
        self.updater.update();
    }

    fn add_covered(self: &Arc<Self>, target: impl AsRef<str>, covered: u32) {
        self.map.add_covered(target, covered);
        self.updater.update();
    }

    fn add_errors(self: &Arc<Self>, target: impl AsRef<str>, errors: u32) {
        self.map.add_errors(target, errors);
        self.updater.update();
    }

    fn started(self: &Arc<Self>, description: String) {
        let meself = self.clone();
        let desc = description.clone();
        self.updater
            .start(description, move || meself.report(&desc));
    }

    fn stopped(self: &Arc<Self>) {
        self.updater.stop();
    }

    fn message(self: &Arc<Self>, msg: String) {
        let meself = self.clone();
        tokio::spawn(async move {
            if let Err(e) = meself.send_message(msg).await {
                error!(meself.log, "Can't send a message to slack"; "error" => e);
            }
        });
    }
}

struct ScheduledUpdater {
    updated: Arc<Notify>,
    stopped: Arc<Notify>,
    log: Logger,
}

impl ScheduledUpdater {
    fn new(log: Logger) -> Self {
        Self {
            updated: Arc::new(Notify::new()),
            stopped: Arc::new(Notify::new()),
            log,
        }
    }

    fn start<F: Fn() + Send + Sync + 'static>(&self, description: String, f: F) {
        let updated = self.updated.clone();
        let stopped = self.stopped.clone();
        let log = self.log.new(o!("desc" => description));
        tokio::spawn(async move {
            let mut timeout = DURATION_LONG;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(timeout) => {
                        trace!(log, "Reporting");
                        f();
                        timeout = DURATION_LONG;
                    }
                    _ = updated.notified() => {
                        trace!(log, "New update, still waiting");
                        timeout = DURATION_SHORT;
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
