#![feature(str_split_once)]

use std::sync::Arc;

use config::Honggfuzz;

use feedback::{Feedback, LoggerClient};
use slog::{crit, debug, error};
use tokio::sync::broadcast::channel;

mod build;
mod checkout;
mod config;
mod error;
mod feedback;
mod hfuzz;
mod report;
mod server;
mod slack;

#[macro_use]
extern crate clap;

#[tokio::main]
async fn main() {
    let matches = clap_app!(ci_fuzz =>
        (version: "1.0")
        (about: "Runs fuzzing in CI")
        (@arg CONFIG: -c --config +takes_value "Sets a custom config file")
        (@arg debug: -d ... "Sets the level of debugging information")
        (@subcommand checkout =>
            (about: "checkout fuzzing repo and target project")
            (@arg DIR: +required "Directory checkout to")
            (@arg REPO: +required "Target project repository")
            (@arg BRANCH: +required "Target project branch")
        )
        (@subcommand hfuzz =>
            (about: "runs hfuzz")
            (@arg DIR: +required "Directory containing honggfuzz project")
            (@arg TEZEDGE: +required "Directory containing tezedge project")
            (@arg CORPUS: -c --corpus "Directory containing honggfuzz corpus")
            (@arg TARGET: ... "Targets to fuzz")
        )
        (@subcommand slack =>
            (about: "runs slack messaging")
            (@arg CHANNEL: +required "Slack channel to post to")
            (@arg TOKEN: +required "Slack authorization token")
        )
        (@subcommand server =>
            (about: "runs CI server")
            (@arg ADDR: -l --listen +takes_value "Address listen to (0.0.0.0:3030 by default)")
            (@arg URL: -u --url +takes_value "Address the server is accessible (ADDR by default)")
            (@arg BRANCHES: -b --branch ... +takes_value "Branches to fuzz")
        )
    )
    .get_matches();

    let log = {
        use slog::{Drain, Level::*};
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain)
            .build()
            .filter_level(match matches.occurrences_of("debug") {
                0 => Info,
                1 => Debug,
                _ => Trace,
            })
            .fuse();

        slog::Logger::root(drain, slog::o!())
    };

    debug!(log, "Starting application");

    let config = matches.value_of("CONFIG").unwrap_or("fuzz-ci.toml");
    let mut config = match config::Config::read(config) {
        Ok(c) => c,
        Err(e) => {
            crit!(log, "Failed to read configuration file {}", config; "error" => e.to_string());
            return;
        }
    };

    if let Some(matches) = matches.subcommand_matches("checkout") {
        let dir = matches.value_of_os("DIR").unwrap();
        let repo = matches.value_of("REPO").unwrap();
        let branch = matches.value_of("BRANCH").unwrap();
        match checkout::checkout(dir, repo, branch, log.clone()).await {
            Ok(_) => (),
            Err(e) => error!(log, "Error occurred"; "error" => e),
        }
    } else if let Some(matches) = matches.subcommand_matches("hfuzz") {
        let dir = matches.value_of_os("DIR").unwrap();
        let root = matches.value_of_os("TEZEDGE").unwrap();
        let corpus = matches.value_of_lossy("CORPUS");
        let targets = matches.values_of_lossy("TARGET").unwrap_or(vec![]);
        let feedback = &config.feedback;
        let hfuzz_config = Honggfuzz::new(None, targets);
        let client = LoggerClient::new("feedback".to_string(), log.clone());
        let feedback = Arc::new(
            Feedback::new(
                feedback,
                Box::new(client),
                &config.reports_path,
                &config.url,
                "reports",
                log.clone(),
            )
            .await
            .unwrap(),
        );

        feedback.started();
        match hfuzz::run(
            dir,
            hfuzz_config,
            root,
            corpus.map(|s| s.into_owned()),
            feedback,
            channel(1).0,
            log.new(slog::o!()),
        )
        .await
        {
            Ok(_) => (),
            Err(e) => error!(log, "Error occurred"; "error" => e),
        }
    } else if let Some(matches) = matches.subcommand_matches("server") {
        if let Some(listen) = matches.value_of("ADDR") {
            config.address = listen.to_string();
        } else if config.address.is_empty() {
            config.address = "0.0.0.0:3030".to_string();
        }

        if let Some(url) = matches.value_of("URL") {
            config.url = Some(url.parse().expect("Failed to parse url"));
        } else if config.url.is_none() {
            config.url = Some(
                format!("http://{}", config.address)
                    .parse()
                    .expect("Failed to parse address as url"),
            );
        }

        if matches.occurrences_of("BRANCH") > 0 {
            config.branches = matches.values_of_lossy("BRANCH").unwrap();
        } else if config.branches.is_empty() {
            config.branches = ["master", "develop"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        }

        server::start(config, log).await;
    } else {
        println!("{}", matches.usage());
    }
}
