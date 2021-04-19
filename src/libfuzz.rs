use std::{ffi::OsStr, io};

use slog::{debug, info};
use tokio::process::Command;

use crate::common;

pub async fn run(
    dir: impl AsRef<OsStr>,
    log: slog::Logger,
) -> io::Result<()> {
    let dir = dir.as_ref();
    info!(log, "Starting libfuzzer"; "dir" => dir.to_str());
    let out = std::fs::File::create(common::new_file(dir, "libfuzzer.out"))?;
    let err = std::fs::File::create(common::new_file(dir, "libfuzzer.err"))?;
    let mut child = Command::new("./run-libfuzzer.sh")
        .env("TERM", "")
        .arg(dir)
        .stdout(out)
        .stderr(err)
        .spawn()?;

    child.wait().await?;

    debug!(log, "libfuzzer run completed successfully");

    Ok(())
}
