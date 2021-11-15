use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::Output,
};

use slog::{debug, trace, FnValue, Logger};
use tokio::{fs::read_dir, process::Command};

use crate::{common::u8_slice_to_string, config::KCov};

#[derive(Clone)]
pub struct Builder {
    corpus: Option<String>,
    kcov: Option<KCov>,
    log: Logger,
}

impl Builder {
    pub fn new(corpus: Option<String>, kcov: Option<KCov>, log: Logger) -> Self {
        Builder { corpus, kcov, log }
    }

    fn error(msg: impl AsRef<str>) -> io::Error {
        io::Error::new(io::ErrorKind::Other, msg.as_ref().to_owned())
    }

    fn os_str_to_string<'a>(os_str: impl AsRef<OsStr>) -> String {
        os_str.as_ref().to_string_lossy().into_owned()
    }

    fn check_output(&self, command: impl AsRef<str>, output: Output) -> io::Result<()> {
        trace!(self.log, "checking output of {}", command.as_ref();
               "stdout" => u8_slice_to_string(&output.stdout),
               "stderr" => u8_slice_to_string(&output.stderr),
               "status" => output.status.code(),
        );
        if !output.status.success() {
            debug!(self.log, "{} returned error", command.as_ref();
                   "stderr" => FnValue(|_| u8_slice_to_string(&output.stderr)),
                   "code" => output.status.code());
            return Err(Self::error(format!("error running {}", command.as_ref())));
        } else {
            debug!(self.log, "{} finished successfully", command.as_ref());
        }

        Ok(())
    }

    async fn find_file(
        &self,
        dir: impl AsRef<Path>,
        pattern: impl AsRef<OsStr>,
    ) -> io::Result<PathBuf> {
        debug!(
            self.log,
            "searching in {:?} for a file starting with {:?}",
            dir.as_ref().to_path_buf().join("debug/target/deps"),
            pattern.as_ref()
        );
        let pattern = Self::os_str_to_string(pattern.as_ref());
        let mut read_dir = read_dir(dir.as_ref().to_path_buf().join("target/debug/deps")).await?;
        while let Some(next) = read_dir.next_entry().await? {
            let file_name = Self::os_str_to_string(next.file_name());
            if next.file_type().await?.is_file()
                && file_name.starts_with(&pattern)
                && !file_name.ends_with(".d")
            {
                return Ok(next.path());
            }
        }
        return Err(Self::error(format!("cannot find file {}", pattern)));
    }

    pub async fn kcov(&self, root: impl AsRef<Path>, dir: impl AsRef<Path>) -> io::Result<()> {
        debug!(self.log, "Running cargo build"; "dir" => dir.as_ref().to_str());

        let KCov { kcov_args } = self
            .kcov
            .as_ref()
            .expect("builder::kcov() shouldn't be called");

        let build_output = Command::new("cargo")
            .args(&["build", "--tests"])
            .current_dir(&dir)
            .output()
            .await?;
        self.check_output("cargo build", build_output)?;

        let test_file = self
            .find_file(&dir, dir.as_ref().file_name().expect("no file name"))
            .await?;
        let mut test_command = Command::new("kcov");
        test_command
            .arg("target/cov")
            .args(kcov_args)
            .arg(test_file)
            .current_dir(dir.as_ref())
            .env(
                "LD_LIBRARY_PATH",
                PathBuf::from(root.as_ref()).join("tezos/sys/lib_tezos/artifacts/"),
            );
        if let Some(corpus) = &self.corpus {
            test_command.env("CORPUS", corpus);
        }

        debug!(self.log, "Running kcov"; "command" => FnValue(|_| format!("{:?}", test_command)));
        self.check_output("kcov", test_command.output().await?)?;

        Ok(())
    }

    pub async fn clean(&self, dir: impl AsRef<Path>) -> io::Result<()> {
        debug!(self.log, "Running cargo clean"; "dir" => dir.as_ref().to_str());
        let output = Command::new("cargo")
            .arg("clean")
            .current_dir(dir)
            .output()
            .await?;

        if output.status.success() {
            debug!(self.log, "cargo build finished successfully");
        } else {
            debug!(self.log, "cargo build returned error";
                   "stderr" => FnValue(|_| std::str::from_utf8(&output.stderr).unwrap_or("<invalid utf8>")),
                   "code" => output.status.code());
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "error running cargo clean",
            ));
        }

        Ok(())
    }

    pub async fn build<D, T>(&self, dir: D, targets: &[T]) -> io::Result<()>
    where D: AsRef<Path>,
          T: AsRef<str>,
    {
        debug!(self.log, "Running cargo hfuzz build"; "dir" => dir.as_ref().to_str());
        let mut args = vec!["hfuzz", "build"];
        for target in targets {
            args.extend_from_slice(&["--bin", target.as_ref()]);
        }
        let output = Command::new("cargo")
            .args(&args)
            .current_dir(dir)
            .output()
            .await?;

        if output.status.success() {
            debug!(self.log, "cargo build finished successfully");
        } else {
            debug!(self.log, "cargo build returned error";
                   "stderr" => FnValue(|_| std::str::from_utf8(&output.stderr).unwrap_or("<invalid utf8>")),
                   "code" => output.status.code());
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "error running cargo hfuzz build",
            ));
        }

        Ok(())
    }
}
