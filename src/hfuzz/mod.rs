use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use slog::{error, info, o, trace, Logger};
use tokio::sync::broadcast::Sender;

use crate::{config::Honggfuzz, feedback::Feedback};

mod target;

async fn _find_reports(path: &impl AsRef<Path>, log: &Logger) -> io::Result<Vec<PathBuf>> {
    let mut result = vec![];
    let mut deq = VecDeque::new();

    info!(log, "searching for reports"; "dir" => path.as_ref().to_str());

    let mut read_dir = tokio::fs::read_dir(path).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        deq.push_back(entry);
    }

    while let Some(entry) = deq.pop_front() {
        let path = entry.path();
        if entry.file_type().await?.is_dir() {
            let mut read_dir = tokio::fs::read_dir(path).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                deq.push_back(entry);
            }
        } else {
            if let Some(name) = path.file_name() {
                if name == "HONGGFUZZ.REPORT.TXT" {
                    trace!(log, "file matched"; "file" => entry.path().to_str());
                    result.push(path);
                }
            }
        }
    }

    Ok(result)
}

pub async fn run(
    dir: impl AsRef<Path>,
    config: Honggfuzz,
    root: impl AsRef<Path>,
    corpus: Option<String>,
    feedback: Arc<Feedback>,
    stop_bc: Sender<()>,
    log: Logger,
) -> io::Result<()> {
    info!(log, "Starting hfuzz"; "dir" => dir.as_ref().to_str());

    let mut handles = vec![];

    for target in config.targets {
        let dir = dir.as_ref().to_path_buf();
        let log = log.new(o!("target" => target.clone()));
        let feedback = feedback.clone();
        let root = root.as_ref().to_path_buf();
        let corpus = corpus.as_ref().map(|c| PathBuf::from(c).join(&target));
        let stop_bc = stop_bc.clone();
        handles.push(tokio::spawn(async move {
            target::Target::new(target, &dir, root, corpus, feedback, stop_bc, log)
                .run()
                .await
        }));
    }

    for handle in handles {
        match handle.await {
            Err(e) => error!(log, "Target panicked: {}", e),
            Ok(Err(e)) => error!(log, "Target error: {}", e),
            Ok(Ok(_)) => (),
        }
    }

    Ok(())
}
