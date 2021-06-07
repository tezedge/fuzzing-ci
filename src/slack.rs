use std::{borrow::Cow, collections::HashMap, io};

use reqwest::header::AUTHORIZATION;
use slog::{Logger, error, info, trace, warn};

use crate::feedback::{FeedbackClient, FeedbackLevel};

const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";

pub struct SlackClient {
    desc: String,
    channel: String,
    token: String,
    level: FeedbackLevel,
    log: Logger,
}

impl FeedbackClient for SlackClient {
    fn message(&self, level: FeedbackLevel, message: &str) {
        if level < self.level {
            info!(self.log, "Skipped message"; "message" => message);
            return;
        }
        let message = format!("{}: {}", self.desc, message);
        let token = self.token.clone();
        let log = self.log.clone();
        let json = self.message_json(&message);
        tokio::spawn(async move {
            trace!(log, "Sending to slack"; "text" => &message);
            let client = reqwest::Client::new();
            let response = client
                .post(POST_MESSAGE_URL)
                .header(AUTHORIZATION, token)
                .json(&json)
                .send()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
                .json::<JsonResponse>()
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

            trace!(log, "Sent to slack"; "response" => format!("{:?}", response));

            if response.ok {
                if let Some(warn) = response.warning {
                    if warn != "missing_charset" {
                        warn!(log, "Posting message"; "warning" => warn);
                    }
                }
                Ok(())
            } else {
                let error = response.error.unwrap_or("unknown error".to_string());
                error!(log, "Posting message"; "error" => &error);
                Err(io::Error::new(io::ErrorKind::Other, error))
            }
        });
    }
}

impl SlackClient {
    pub fn new(
        desc: impl AsRef<str>,
        channel: impl AsRef<str>,
        token: impl AsRef<str>,
        level: FeedbackLevel,
        log: Logger,
    ) -> Self {
        Self {
            desc: desc.as_ref().into(),
            channel: channel.as_ref().into(),
            token: format!("Bearer {}", token.as_ref()),
            level,
            log,
        }
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
}

#[derive(serde::Deserialize, Debug)]
pub struct JsonResponse {
    ok: bool,
    warning: Option<String>,
    error: Option<String>,
}

/*
impl SlackFeedback {
    pub async fn start(config: &Slack, log: Logger) -> io::Result<Self> {
        let meself = SlackFeedback {
            client: Arc::new(SlackClient::new(&config.channel, &format!("Bearer {}", config.token), log.clone())),
            map: Arc::new(SharedFeedbackMap::new()),
            updater: ScheduledUpdater::new(log.clone()),
        };
        Ok(meself)
    }

    fn report(&self, description: &String) {
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
        let client = self.client.clone();
        tokio::spawn(async move {
            if let Err(e) = client.send_message(r).await {
                error!(client.log, "Can't send a message to slack"; "error" => e);
            }
        });
    }
}

const DURATION_SHORT: Duration = Duration::from_secs(60);
const DURATION_LONG: Duration = Duration::from_secs(3600);

impl Feedback for SlackFeedback {
    fn set_total(&self, target: &str, total: u32) {
        self.map.set_total(target, total);
        self.updater.update();
    }

    fn add_covered(&self, target: &str, covered: u32) {
        self.map.add_covered(target, covered);
        self.updater.update();
    }

    fn add_errors(&self, target: &str, errors: u32) {
        self.map.add_errors(target, errors);
        self.updater.update();
    }

    fn started(&self, description: String) {
        self.message(format!("Started {}", description));
    }

    fn stopped(&self) {
        self.updater.stop();
    }

    fn message(&self, msg: String) {
        let client = self.client.clone();
        tokio::spawn(async move {
            if let Err(e) = client.send_message(msg).await {
                error!(client.log, "Can't send a message to slack"; "error" => e);
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

*/
