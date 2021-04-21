use std::{
    borrow::Cow,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{mpsc::channel, Arc},
    time::Duration,
};

use notify::DebouncedEvent;
use slog::{debug, error, info, o, trace, Logger};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt},
    process::Command,
    sync::broadcast::Sender,
};

use crate::feedback::Feedback;

pub struct Target<T> {
    name: String,
    dir: PathBuf,
    hfuzz_run_args: String,
    ld_path: PathBuf,
    feedback: Arc<T>,
    stop_bc: Sender<()>,
    log: Logger,
}

impl<T: Feedback + Send + Sync + 'static> Target<T> {
    pub fn new<'a>(
        name: impl Into<Cow<'a, str>>,
        dir: impl Into<Cow<'a, Path>>,
        root: impl AsRef<Path>,
        corpus: Option<PathBuf>,
        feedback: Arc<T>,
        stop_bc: Sender<()>,
        log: Logger,
    ) -> Self {
        let name = name.into().into_owned();
        let mut hfuzz_run_args = "-F 1048576".to_string(); // max input size
        if let Some(corpus) = corpus {
            hfuzz_run_args += &format!(" -i {}", corpus.to_string_lossy());
        }
        let ld_path = root
                .as_ref()
                .to_path_buf()
                .join("tezos/sys/lib_tezos/artifacts/");
        Self {
            name,
            dir: dir.into().into_owned(),
            hfuzz_run_args,
            ld_path,
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
            .env("LD_LIBRARY_PATH", &self.ld_path)
            .env("HFUZZ_RUN_ARGS", hfuzz_run_args);

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
        feedback: Arc<T>,
        mut read: (impl AsyncBufRead + Unpin + Send),
        log: Logger,
    ) {
        let mut edges = 0;
        let mut line = String::new();
        while {
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
            }
            line.clear();
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

    fn watch_report(&self) -> io::Result<()> {
        use notify::{watcher, RecursiveMode, Watcher};
        let (tx, rx) = channel();
        let mut watcher = watcher(tx, Duration::from_secs(10)).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("cannot create watcher: {}", e),
            )
        })?;
        let mut path = PathBuf::from(&self.dir);
        path.push("hfuzz_workspace");
        path.push(&self.name);
        watcher
            .watch(path.clone(), RecursiveMode::NonRecursive)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "cannot watch path {}: {}",
                        path.to_str().unwrap_or("<invalid path>"),
                        e
                    ),
                )
            })?;
        let log = self.log.new(o!("file_watcher" => ()));
        let feedback = self.feedback.clone();
        let target = self.name.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                match rx.recv() {
                    Ok(event) => {
                        trace!(log, "FS event: {:?}", event);
                        match event {
                            DebouncedEvent::Create(path) | DebouncedEvent::Write(path) => {
                                if let Some(path) = path.file_name() {
                                    if path == "HONGGFUZZ.REPORT.TXT" {
                                        feedback.add_errors(&target, 1);
                                    }
                                }
                            }
                            DebouncedEvent::Remove(path) => match path.file_name() {
                                Some(path) if path.to_str() == Some(&target) => {
                                    break;
                                }
                                _ => (),
                            },
                            _ => (),
                        }
                    }
                    Err(e) => {
                        error!(log, "Error occured: {}", e);
                        break;
                    }
                }
            }
            let _watcher = watcher;
        });

        Ok(())
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
        self.watch_report()?;
        tokio::select! {
            _ = Self::filter_output(self.name.clone(), self.feedback.clone(), stderr, self.log.clone()) => (),
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
