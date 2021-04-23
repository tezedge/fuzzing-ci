use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use derive_new::new;
use failure::Error;
use serde::{Deserialize, Serialize};
use slog::{debug, error, info, o, trace, warn, Logger};
use tokio::sync::{
    broadcast::{channel, Sender},
    Mutex,
};
use warp::Filter;

use crate::{build::Builder, common, config::{self, Config}, feedback::{Feedback, FeedbackClient, LoggerClient}, slack::SlackClient};

const RUN_PATH: &str = "run";

#[derive(Serialize, Deserialize)]
struct PingEvent {
    zen: String,
}

#[derive(Serialize, Deserialize)]
struct PushEvent {
    #[serde(alias = "ref")]
    ref_: String,
    repository: Repository,
    commits: Vec<Commit>,
    head_commit: Option<Commit>,
}

#[derive(Serialize, Deserialize)]
struct Repository {
    ssh_url: String,
    url: String,
}

#[derive(Serialize, Deserialize)]
struct Commit {
    id: String,
    message: String,
    timestamp: String,
    author: Author,
}

#[derive(Serialize, Deserialize)]
struct Author {
    name: String,
    email: String,
    username: String,
}

fn get_stop_bc(
    notifies: Arc<RwLock<HashMap<String, Sender<()>>>>,
    branch: &String,
    log: &Logger,
) -> Sender<()> {
    {
        let map = notifies.read().unwrap();
        if let Some(notify) = map.get(branch) {
            trace!(
                log,
                "Found broadcast notification, notifying it to stop previous run"
            );
            match notify.send(()) {
                Ok(_) => debug!(log, "Notification is sent"),
                Err(e) => warn!(log, "Notification is not sent"; "error" => e.to_string()),
            };
            return notify.clone();
        }
    }

    trace!(log, "Creating new broadcast channel");
    let notify = channel(1).0;
    let mut map = notifies.write().unwrap();
    map.insert(branch.clone(), notify.clone());
    trace!(log, "Added new broadcast channel");
    notify
}

async fn copy_cov_files(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    log: &Logger,
) -> io::Result<()> {
    let mut src = PathBuf::from(src.as_ref());
    src.push("target/cov/.");

    std::fs::create_dir_all(&dst)?;

    debug!(
        log,
        "Copy files from {} to {}",
        src.to_str().unwrap_or("<invalid utf8>"),
        dst.as_ref().to_str().unwrap_or("<invalid utf8>")
    );

    tokio::process::Command::new("cp")
        .arg("-r")
        .arg(src)
        .arg(dst.as_ref())
        .status()
        .await?;

    Ok(())
}

async fn run_fuzzers<'a>(
    url: String,
    builder: Arc<Mutex<Builder>>,
    config: Config,
    feedback: Arc<Feedback>,
    reports_path: &'a Path,
    branch: &'a str,
    stop_bc: Sender<()>,
    log: Logger,
) -> Result<(), Error> {
    slog::info!(log, "A branch has been checked out"; "branch" => branch);
    let tempdir = tempfile::tempdir()?;
    let path = tempdir.path();
    super::checkout::checkout(path, url, &branch, log.new(slog::o!("stage" => "checkout"))).await?;
    let mut handles = vec![];
    let tezedge_root = tempdir.path().to_path_buf().join("code/tezedge");

    if config.kcov.is_some() {
        debug!(log, "Generating coverage reports");
        let mut some = false;
        for (name, conf) in &config.honggfuzz {
            let path = PathBuf::from(path).join(conf.path.as_ref().unwrap_or(&name));

            let builder = builder.lock().await;

            match builder.kcov(&tezedge_root, &path).await {
                Ok(_) => {
                    if let Err(e) = copy_cov_files(
                        &path,
                        config.reports_path.join(reports_path).join(&name),
                        &log,
                    )
                    .await
                    {
                        error!(log, "Error copying reports: {}", e);
                    } else {
                        some = true;
                    }
                }
                Err(e) => {
                    error!(log, "Error running kcov: {}", e);
                }
            }
        }
        if some {
            if let Some(url) = config.url {
                feedback.message(format!(
                    "Coverage reports are ready: {}",
                    common::reports_url(&url, reports_path)?
                ));
            }
        }
    }

    debug!(log, "Building fuzzing projects");
    for (name, conf) in &config.honggfuzz {
        let path = PathBuf::from(path).join(conf.path.as_ref().unwrap_or(&name));
        let _ = builder.lock().await.clean(&path).await;
        let _ = builder.lock().await.build(&path).await;
    }

    for (name, conf) in config.honggfuzz {
        if conf.targets.is_empty() {
            continue;
        }
        let path = PathBuf::from(path).join(conf.path.as_ref().unwrap_or(&name));
        let feedback = feedback.clone();
        let log = log.new(slog::o!("stage" => "hfuzz"));
        let tezedge_root = tezedge_root.clone();
        let corpus = config.corpus.clone();
        let stop_bc = stop_bc.clone();
        handles.push(tokio::spawn(async move {
            super::hfuzz::run(path, conf, tezedge_root, corpus, feedback, stop_bc, log).await
        }));
    }
    feedback.started();
    for handle in handles {
        match handle.await {
            Ok(r) => match r {
                Ok(_) => (),
                Err(e) => error!(log, "Fuzzer finished with error: {}", e),
            },
            Err(e) => error!(log, "Fuzzer panicked with error: {}", e),
        }
    }
    info!(
        log,
        "Cleaning up directory {}",
        tempdir.path().to_str().unwrap_or("<invalid utf8>")
    );
    tempdir.close()?;
    Ok(())
}

/// Unique run ID, containing commit message, commit ID, committer and this run timestamp
fn get_run_id(commit: &Commit) -> String {
    // 5-char commit id
    let (id, _) = commit.id.split_at(5);
    // first line of the commit message
    let message = commit.message.split('\n').next().unwrap();
    format!(
        "{} - {} by {} at {}",
        message,
        id,
        commit.author.username,
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
    )
}

async fn create_feedback(
    config: &config::Config,
    description: &str,
    reports_loc: &Path,
    stop_bc: &Sender<()>,
    log: &Logger,
) -> Arc<Feedback> {
    let client: Box<dyn FeedbackClient + Sync + Send> = if let Some(config) = &config.slack {
        Box::new(SlackClient::new(
            description,
            &config.channel,
            &config.token,
            log.clone(),
        ))
    } else {
        Box::new(LoggerClient::new(description, log.clone()))
    };
    let feedback = Feedback::new(
        &config.feedback,
        client,
        &config.reports_path,
        &config.url,
        &reports_loc,
        log.clone(),
    )
    .await
    .expect("can't create feedback");
    let feedback = Arc::new(feedback);
    {
        let feedback = feedback.clone();
        let mut stop = stop_bc.subscribe();
        let log = log.clone();
        tokio::spawn(async move {
            if let Err(e) = stop.recv().await {
                error!(log, "Error receiving broadcast"; "error" => e.to_string());
            }
            feedback.stopped();
        });
    }
    feedback
}

async fn push_hook(
    push: PushEvent,
    config: Config,
    builder: Arc<Mutex<Builder>>,
    stop_bcs: Arc<RwLock<HashMap<String, Sender<()>>>>,
    log: Logger,
) -> Result<impl warp::Reply, warp::Rejection> {
    let url = push.repository.url;
    let branch = match push.ref_.strip_prefix("refs/heads/") {
        Some(branch) => branch.to_string(),
        None => return Err(warp::reject()),
    };
    trace!(log, "Push event"; "repo" => &url, "branch" => &branch);
    if config.branches.contains(&branch) {
        let log = log.new(o!("branch" => branch.clone()));
        trace!(log, "Starting fuzzing on branch {}", branch);
        let stop_bc = get_stop_bc(stop_bcs, &branch, &log);

        let run_id = if let Some(commit) = &push.head_commit {
            get_run_id(commit)
        } else if let Some(commit) = push.commits.first() {
            get_run_id(commit)
        } else {
            "no commit".to_string()
        };

        let reports_loc = common::new_local_path(&[&branch, &run_id]);
        let description = format!("Branch _{}_, {}", branch, run_id);

        let feedback = create_feedback(&config, &description, &reports_loc, &stop_bc, &log).await;
        feedback.message("Preparing for fuzzing".to_string());
        trace!(log, "Spawning fuzzer");
        tokio::spawn(async move {
            let mut stop = stop_bc.subscribe();
            tokio::select! {
                res = run_fuzzers(url, builder, config, feedback, &reports_loc, &branch, stop_bc.clone(), log.clone()) => {
                    match res {
                        Ok(_) => (),
                        Err(e) => error!(log, "Error running fuzzers"; "error" => e.to_string()),
                    }
                }
                _ = stop.recv() => {
                    info!(log, "Stopping fuzzers");
                }
            }
        });
    } else {
        debug!(log, "Skipping branch");
    }
    Ok(warp::reply())
}

#[derive(Serialize)]
struct BranchReports {
    name: String,
    reports: Vec<String>,
}

impl BranchReports {
    pub fn read(dir: impl AsRef<Path>, branches: Vec<String>, log: Logger) -> Vec<Self> {
        let dir = dir.as_ref().to_path_buf();
        branches
            .iter()
            .map(|name| {
                let dir = dir.join(name);
                debug!(log, "Inspecting {:?}", dir);
                let read_dir = match std::fs::read_dir(dir) {
                    Ok(read_dir) => read_dir,
                    Err(_) => return None,
                };
                let mut reports = read_dir
                    .map(|res| {
                        res.map(|e| e.path().file_name().unwrap().to_string_lossy().into_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                reports.sort();
                debug!(log, "Read content {}", reports.join(", "));
                Some(BranchReports {
                    name: name.clone(),
                    reports,
                })
            })
            .filter_map(|s| s)
            .collect()
    }
}

const REPORTS: &str = r#"
<h1>Fuzzing coverage reports</h1>
{{#each this}}
<details>
  <summary>{{name}}</summary>
    <ul>
    {{#each reports}}
      <li><a href="./{{../name}}/{{this}}/">{{this}}</a></li>
    {{/each}}
    </ul>
</details>
{{/each}}
"#;

#[derive(Serialize, new)]
struct Report {
    branch: String,
    time: String,
    projects: Vec<String>,
}

const REPORT: &str = r#"
<h1>Coverage report {{time}} for branch {{branch}}</h1>
<table>
<tr><th>Fuzzing project</th><tr>
{{#each projects}}
<tr><td><a href="./{{this}}/index.html">{{this}}</a></td></tr>
{{/each}}
</table>
"#;

use handlebars::Handlebars;

fn render<T>(name: &'static str, value: T, hbs: Arc<Handlebars>) -> impl warp::Reply
where
    T: Serialize,
{
    let render = hbs
        .render(name, &value)
        .unwrap_or_else(|err| err.to_string());
    warp::reply::html(render)
}

/*
fn branches(dir: String) -> HashMap<String, Vec<String>> {
    let mut branches = std::fs::read_dir(dir)
        .unwrap()
        .map(|res| res.map(|e| e.path().file_name().unwrap().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    branches.sort();
    branches
}
 */

pub(crate) async fn start(config: Config, log: slog::Logger) {
    pretty_env_logger::init();

    info!(log, "Starting server"; "address" => &config.address);
    let addr = match config.address.parse::<SocketAddr>() {
        Ok(a) => a,
        Err(e) => {
            error!(log, "Cannot parse address {}", config.address; "error" => e.to_string());
            return;
        }
    };

    let ping_log = log.new(slog::o!("event" => "ping"));
    let ping = warp::header::exact("X-GitHub-Event", "ping")
        .and(warp::body::json::<PingEvent>())
        .map(move |body| {
            debug!(ping_log, "Incoming ping"; "body" => serde_json::to_string(&body).unwrap());
            warp::reply()
        });

    let push = {
        let config = config.clone();
        let builder = Arc::new(Mutex::new(Builder::new(
            config.corpus.clone(),
            config.kcov.clone(),
            log.new(o!("component" => "builder")),
        )));
        let notifies = Arc::new(RwLock::new(HashMap::new()));
        let push_log = log.new(slog::o!("event" => "push"));
        warp::header::exact("X-GitHub-Event", "push")
            .and(warp::body::json::<PushEvent>())
            .and(warp::any().map(move || config.clone()))
            .and(warp::any().map(move || builder.clone()))
            .and(warp::any().map(move || notifies.clone()))
            .and(warp::any().map(move || push_log.clone()))
            .and_then(push_hook)
    };

    let mut hb = Handlebars::new();
    hb.register_template_string("reports", REPORTS).unwrap();
    hb.register_template_string("report", REPORT).unwrap();
    let hb = Arc::new(hb);

    let reports = {
        let mut branches = config.branches.clone();
        branches.sort();
        let dir = PathBuf::from(&config.reports_path);
        let log = log.clone();
        let reports = move |hb| {
            let reports = BranchReports::read(dir.clone(), branches.clone(), log.clone());
            render("reports", reports, hb)
        };
        let hb = hb.clone();
        warp::path("reports")
            .and(warp::path::end())
            .and(warp::any().map(move || hb.clone()))
            .map(reports)
    };

    let report = {
        let mut projects = config.honggfuzz.keys().cloned().collect::<Vec<_>>();
        projects.sort();
        let hb = hb.clone();
        warp::path!("reports" / String / String).map(move |branch, time| {
            let report = Report::new(branch, time, projects.clone());
            render("report", report, hb.clone())
        })
    };

    let coverage = reports.or(warp::path!("reports" / ..).and(warp::fs::dir(config.reports_path)));

    let webhook_routes = warp::post().and(warp::path(RUN_PATH)).and(ping.or(push));
    let reports_routes = report.or(coverage);
    let routes = reports_routes.or(webhook_routes);

    warp::serve(routes).run(addr).await
}
