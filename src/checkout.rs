use std::{ffi::OsStr, io};

use slog::{info, FnValue};
use tokio::process::Command;

pub async fn checkout(
    dir: impl AsRef<OsStr>,
    url: impl AsRef<str>,
    branch: impl AsRef<str>,
    log: slog::Logger,
) -> io::Result<()> {
    let dir = dir.as_ref();
    info!(log, "Checking out"; "dir" => dir.to_str(), "url" => url.as_ref(), "branch" => branch.as_ref());
    let output = Command::new("./checkout.sh")
        .arg(dir)
        .arg(url.as_ref())
        .arg(branch.as_ref())
        .output()
        .await?;

    slog::debug!(log, "Checkout command completes successfully"; "output" => FnValue(|_| std::str::from_utf8(&output.stderr).unwrap_or("<invalid utf8>")));

    Ok(())
}
