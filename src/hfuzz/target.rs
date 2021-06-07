use std::{borrow::Cow, collections::HashMap, io, path::{Path, PathBuf}, process::Stdio, sync::Arc};

use slog::{FnValue, Logger, debug, error, info, trace};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt},
    process::Command,
    sync::broadcast::Sender,
};

use crate::{config::HonggfuzzConfig, feedback::Feedback};

pub struct Target {
    name: String,
    dir: PathBuf,
    env: HashMap<String, String>,
    hfuzz_run_args: String,
    feedback: Arc<Feedback>,
    stop_bc: Sender<()>,
    log: Logger,
}

impl Target {
    pub fn new<'a>(
        name: impl Into<Cow<'a, str>>,
        dir: impl Into<Cow<'a, Path>>,
        env: HashMap<String, String>,
        hfuzz_config: &HonggfuzzConfig,
        corpus: Option<PathBuf>,
        feedback: Arc<Feedback>,
        stop_bc: Sender<()>,
        log: Logger,
    ) -> Self {
        let name = name.into().into_owned();
        let mut hfuzz_run_args = hfuzz_config.run_args.clone();
        if let Some(corpus) = corpus {
            hfuzz_run_args += &format!(" -i {}", corpus.to_string_lossy());
        }
        Self {
            name,
            dir: dir.into().into_owned(),
            env,
            hfuzz_run_args,
            feedback,
            stop_bc,
            log,
        }
    }

    #[inline]
    fn hfuzz_run_base(&self, hfuzz_run_args: impl AsRef<str>) -> Command {
        let hfuzz_run_args = format!("{} {}", hfuzz_run_args.as_ref(), self.hfuzz_run_args);
        let mut command = Command::new("cargo");
        command
            .args(&["hfuzz", "run"])
            .arg(&self.name)
            .current_dir(&self.dir)
            .kill_on_drop(true)
            .env("HFUZZ_RUN_ARGS", &hfuzz_run_args)
            .envs(&self.env);

        trace!(self.log, "hfuzz command: {:?}", command;
               "HFUZZ_RUN_ARGS" => FnValue(|_| format!("{:?}", &hfuzz_run_args)),
               "env" => FnValue(|_| format!("{:?}", &self.env)));

        command
    }

    #[inline]
    fn hfuzz_run(&self) -> Command {
        self.hfuzz_run_base("-v")
    }

    #[inline]
    fn hfuzz_run_min(&self) -> Command {
        self.hfuzz_run_base("-v -N 1 -n 1")
    }

    async fn filter_output(
        name: String,
        dir: PathBuf,
        feedback: Arc<Feedback>,
        mut read: (impl AsyncBufRead + Unpin + Send),
        log: Logger,
    ) {
        let mut edges = 0;
        let mut line = String::new();
        while {
            line.clear();
            match read.read_line(&mut line).await {
                Ok(s) => s,
                Err(e) => {
                    error!(log, "error in hfuzz output filter"; "error" => e);
                    0
                }
            }
        } > 0
        {
            if line.starts_with("Sz:") {
                let e = match line.split("/").skip(8).next() {
                    Some(e) => e,
                    None => {
                        error!(log, "error in hfuzz output filter");
                        break;
                    }
                };

                if e == "0" {
                    continue;
                }
                let e: u32 = match e.parse() {
                    Ok(e) => e,
                    Err(e) => {
                        error!(log, "error in hfuzz output filter"; "error" => e.to_string());
                        break;
                    }
                };
                feedback.add_covered(&name, e);
                edges += e;
                trace!(log, "coverage update"; "edges" => edges);
            } else if line.starts_with("Crash: saved as '") {
                if let Some(file) = line["Crash: saved as '".len()..].split_terminator("'").next() {
                    let file = dir.join(file);
                    let file = file.to_string_lossy();
                    feedback.add_error(&name, &file)
                } else {
                    error!(log, "Cannot parse error line"; "line" => &line)
                }
            }
        }
    }

    async fn get_total_coverage(&self) -> io::Result<u32> {
        trace!(self.log, "Run the target shortly to get target coverage"; "target" => &self.name);
        let output = self
            .hfuzz_run_min()
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if !output.status.success() {
            error!(self.log, "Error running target"; "code" => output.status.code());
            debug!(self.log, "Error running target"; "output" => std::str::from_utf8(&output.stderr).unwrap_or("<invalid utf8>"));
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("error running target {}", self.name),
            ));
        }
        let last = output
            .stderr
            .split(|ch| *ch == 0x0a)
            .rev()
            .find(|s| !s.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no output"))?;
        let last = std::str::from_utf8(last)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "invalid utf8"))?;
        trace!(self.log, "last line found"; "line" => last);

        let edge_nr = last
            .split_once("guard_nb:")
            .map(|(_, s)| s.split_once(" "))
            .flatten()
            .map(|(s, _)| s)
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "cannot get edge nr"))?;
        let edge_nr: u32 = edge_nr
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "cannot parse"))?;
        trace!(self.log, "edge nr"; "_" => edge_nr);

        Ok(edge_nr)
    }

    pub async fn run(&self) -> io::Result<()> {
        let total = self.get_total_coverage().await?;
        self.feedback.set_total(&self.name, total);

        trace!(self.log, "Run the target");
        let mut child = self
            .hfuzz_run()
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "cannot get stderr"))?;
        let stderr = tokio::io::BufReader::new(stderr);
        let mut stop = self.stop_bc.subscribe();
        tokio::select! {
            _ = Self::filter_output(self.name.clone(), self.dir.clone(), self.feedback.clone(), stderr, self.log.clone()) => (),
            _ = stop.recv() => {
                debug!(self.log, "Terminating target {}", self.name);
                child.kill().await?;
            }
        };

        let res = child.wait().await?;
        info!(self.log, "Finished target {}", self.name; "status" => res.code());

        Ok(())
    }
}
