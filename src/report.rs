use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use handlebars::Handlebars;
use slog::{debug, info, trace, Logger};
use tokio::{
    fs::{read_dir, File},
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::error::Error;

#[derive(Clone, Copy, derive_new::new, Default, serde::Serialize, serde::Deserialize)]
pub struct TargetStatus {
    pub total: u32,
    pub covered: u32,
    pub errors: u32,
}

#[derive(Clone, Copy, derive_new::new, Default, serde::Serialize, serde::Deserialize)]
pub struct TargetStatusDelta {
    pub total: i32,
    pub covered: i32,
    pub errors: i32,
}

#[derive(Clone, derive_new::new, Default, serde::Serialize, serde::Deserialize)]
struct TargetStatusDiff {
    name: String,
    curr: TargetStatus,
    prev: Option<TargetStatus>,
    delta: Option<TargetStatusDelta>,
}

impl From<(TargetStatus, TargetStatus)> for TargetStatusDelta {
    fn from((curr, prev): (TargetStatus, TargetStatus)) -> Self {
        Self {
            total: curr.total as i32 - prev.total as i32,
            covered: curr.covered as i32 - prev.covered as i32,
            errors: curr.errors as i32 - prev.errors as i32,
        }
    }
}

impl From<(String, TargetStatus, Option<TargetStatus>)> for TargetStatusDiff {
    fn from((name, curr, prev): (String, TargetStatus, Option<TargetStatus>)) -> Self {
        let delta = prev.map(|s| (curr, s).into());
        Self {
            name,
            curr,
            prev,
            delta,
        }
    }
}

pub type FuzzingStatus = HashMap<String, TargetStatus>;

use static_init::dynamic;

#[dynamic]
static HANDLEBARS: Handlebars<'static> = {
    let mut hb = Handlebars::new();
    hb.register_template_string("report", REPORT)
        .expect("error in template");
    hb
};

const REPORT: &str = r#"
<html>
  <!DOCTYPE html>
<html>
<head>
<style>
table {
  font-family: arial, sans-serif;
  border-collapse: collapse;
  width: 60%;
}

td, th {
  border: 1px solid #dddddd;
  text-align: left;
  padding: 8px;
}

tr:nth-child(even) {
  background-color: #dddddd;
}
</style>
</head>
<body>

  <table>
    <tr>
      <th>Fuzzing target</th>
      <th>Current coverage</th>
      <th>Previous coverage</th>
      <th>Delta</th>
    </tr>
    {{#each this}}
    <tr>
      <td>{{name}}</td>
      <td>{{curr.covered}}/{{curr.total}}</td>
      {{#if prev}}
      <td>{{prev.covered}}/{{prev.total}}</td>
      <td>{{delta.covered}}/{{delta.total}}</td>
      {{else}}
      <td>N/A</td>
      <td>N/A</td>
      {{/if}}
    </tr>
    {{/each}}
  </table>
  </body>
</html>
"#;

const STATUS_FILE: &str = "status.json";
const REPORT_FILE: &str = "report.html";

pub struct Report {
    current: PathBuf,
    previous: Option<FuzzingStatus>,
    log: Logger,
}

impl Report {
    pub async fn new(dir: impl AsRef<Path>, log: Logger) -> Result<Self, Error> {
        let current = dir.as_ref().canonicalize()?;
        info!(
            log,
            "Initializing reporting in {}",
            current.to_string_lossy()
        );
        let reports = current.parent().unwrap();
        let previous = Self::find_previous(&reports, &dir, &log).await?;

        let previous = if let Some(previous) = previous {
            Some(Self::load(&previous.join(STATUS_FILE)).await?)
        } else {
            None
        };

        Ok(Self {
            current,
            previous,
            log,
        })
    }

    async fn find_previous(
        reports: impl AsRef<Path>,
        current: impl AsRef<Path>,
        log: &Logger,
    ) -> Result<Option<PathBuf>, Error> {
        trace!(
            log,
            "locating previous report in {}",
            reports.as_ref().to_string_lossy()
        );
        let mut read_dir = match read_dir(reports).await {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let mut latest: Option<(PathBuf, SystemTime)> = None;
        while let Some(entry) = read_dir.next_entry().await? {
            if entry.file_type().await?.is_dir() && entry.path() != current.as_ref() {
                let (path, created) = (entry.path(), entry.metadata().await?.created()?);
                if let Some(ref latest) = latest {
                    if latest.1 > created {
                        continue;
                    }
                }
                latest = Some((path, created));
            }
        }
        trace!(log, "found {:?}", latest);
        Ok(latest.map(|o| o.0))
    }

    async fn save(status: &FuzzingStatus, file: impl AsRef<Path>) -> Result<(), Error> {
        File::create(file)
            .await?
            .write_all(&serde_json::to_vec_pretty(&status)?)
            .await?;
        Ok(())
    }

    async fn load(file: impl AsRef<Path>) -> io::Result<FuzzingStatus> {
        let mut json = vec![];
        File::open(file).await?.read_to_end(&mut json).await?;
        Ok(serde_json::from_slice(&json)?)
    }

    pub async fn update(&self, status: &FuzzingStatus) -> Result<(), Error> {
        debug!(
            self.log,
            "Updating current fuzzing status in {}",
            self.current.join(STATUS_FILE).to_string_lossy()
        );
        Self::save(status, &self.current.join(STATUS_FILE)).await
    }

    fn get_diff(&self, name: &String, curr: &TargetStatus) -> TargetStatusDiff {
        let prev: Option<TargetStatus> = self
            .previous
            .as_ref()
            .map(|prev| prev.get(name))
            .flatten()
            .cloned();
        (name.clone(), *curr, prev).into()
    }

    pub async fn generate_report(&self, status: &FuzzingStatus) -> Result<(), Error> {
        debug!(
            self.log,
            "Generating coverage report in {}",
            self.current.join(REPORT_FILE).to_string_lossy()
        );
        let mut diff: Vec<TargetStatusDiff> =
            status.iter().map(|(k, s)| self.get_diff(k, s)).collect();
        diff.sort_by(|a, b| a.name.cmp(&b.name));
        let report = HANDLEBARS.render("report", &diff)?;
        File::create(self.current.join(REPORT_FILE))
            .await?
            .write_all(report.as_bytes())
            .await?;
        Ok(())
    }
}
