use std::{
    collections::HashMap,
    ffi::OsStr,
    fmt::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use failure::ResultExt;
use handlebars::Handlebars;
use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use reqwest::Url;
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

/// Fuzzing target coverage difference
#[derive(Clone, derive_new::new, Default, serde::Serialize, serde::Deserialize)]
struct TargetStatusDiff {
    /// target name
    name: String,
    /// current coverage
    curr: TargetStatus,
    /// previously reported coverage
    prev: Option<TargetStatus>,
    /// delta with previously reported coverage
    delta: Option<TargetStatusDelta>,
    /// previous run coverage
    prev_run: Option<TargetStatus>,
    /// delta with previous run coverage
    delta_run: Option<TargetStatusDelta>,
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

impl
    From<(
        String,
        TargetStatus,
        Option<TargetStatus>,
        Option<TargetStatus>,
    )> for TargetStatusDiff
{
    fn from(
        (name, curr, prev, prev_run): (
            String,
            TargetStatus,
            Option<TargetStatus>,
            Option<TargetStatus>,
        ),
    ) -> Self {
        let delta = prev.map(|s| (curr, s).into());
        let delta_run = prev_run.map(|s| (curr, s).into());
        Self {
            name,
            curr,
            prev,
            delta,
            prev_run,
            delta_run,
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
      <th>Coverage from previous run</th>
      <th>Delta with previous run</th>
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
      {{#if prev_run}}
      <td>{{prev_run.covered}}/{{prev_run.total}}</td>
      <td>{{delta_run.covered}}/{{delta_run.total}}</td>
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

const STATUS_FILE: &str = "hfuzz-status.toml";
const REPORT_FILE: &str = "hfuzz-report/index.html";

pub struct Report {
    reports_dir: PathBuf,
    reports_url: Option<Url>,
    previous: Option<FuzzingStatus>,
    log: Logger,
}

impl Report {
    pub async fn new<'a>(
        reports_dir: &'a Path,
        reports_url: &'a Option<Url>,
        current_path: &'a Path,
        log: Logger,
    ) -> Result<Self, Error> {
        let reports_dir = reports_dir.join(&current_path);
        info!(
            log,
            "Initializing reporting in {}",
            reports_dir.to_string_lossy()
        );

        let parent = reports_dir.parent();
        let previous = if let Some(parent) = parent {
            Self::find_previous(&parent, &reports_dir, &log).await?
        } else {
            None
        };
        let previous = if let Some(previous) = previous {
            Self::load(&previous.join(STATUS_FILE)).await?
        } else {
            None
        };

        let reports_url = if let Some(reports_url) = reports_url {
            let mut reports_url = reports_url.clone();
            for segment in current_path {
                reports_url = reports_url.join(&(Self::escape_segment(segment) + "/"))?
            }
            Some(reports_url)
        } else {
            None
        };

        Ok(Self {
            reports_dir,
            reports_url,
            previous,
            log,
        })
    }

    fn escape_segment(segment: &OsStr) -> String {
        percent_encode(
            segment.to_string_lossy().as_ref().as_bytes(),
            NON_ALPHANUMERIC,
        )
        .to_string()
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
            if entry.file_type().await?.is_dir()
                && entry.path() != current.as_ref()
                && entry.path().join(STATUS_FILE).exists()
            {
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

    fn serialize(status: &FuzzingStatus) -> Result<Vec<u8>, Error> {
        //serde_json::to_vec_pretty(&status)
        Ok(toml::to_vec(status)?)
    }

    fn deserialize(bytes: &[u8]) -> Result<FuzzingStatus, Error> {
        //serde_json::from_slice(bytes)
        Ok(toml::from_slice(bytes)?)
    }

    async fn save(data: &[u8], file: impl AsRef<Path>) -> Result<(), Error> {
        if let Some(parent) = file.as_ref().parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        File::create(file).await?.write_all(data).await?;
        Ok(())
    }

    async fn save_status(status: &FuzzingStatus, file: impl AsRef<Path>) -> Result<(), Error> {
        Self::save(&Self::serialize(status)?, file).await
    }

    async fn load(file: impl AsRef<Path>) -> Result<Option<FuzzingStatus>, Error> {
        if !file.as_ref().exists() {
            return Ok(None);
        }
        let mut json = vec![];
        File::open(file).await?.read_to_end(&mut json).await?;
        Ok(Some(Self::deserialize(&json)?))
    }

    /// Updates current status and generates report basing on it and the previous status.
    ///
    /// Returns summary of what has been changed (new edges since previous report
    /// or different coverage compared to the previous run).
    pub async fn update(&self, status: &FuzzingStatus) -> Result<String, failure::Error> {
        debug!(self.log, "Updating current fuzzing status",);

        // load previously reported status and save the new one
        let status_file = self.reports_dir.join(STATUS_FILE);
        let prev_status = Self::load(&status_file)
            .await
            .with_context(|e| format!("error loading {}: {}", status_file.to_string_lossy(), e))?;
        Self::save_status(status, &status_file)
            .await
            .with_context(|e| format!("error saving {}: {}", status_file.to_string_lossy(), e))?;

        // construct report table containing current and reference data
        let mut diff: Vec<TargetStatusDiff> = status
            .iter()
            .map(|(k, s)| self.get_diff(k, s, &prev_status))
            .collect();
        diff.sort_by(|a, b| a.name.cmp(&b.name));
        let report = HANDLEBARS.render("report", &diff)?;
        let report_file = self.reports_dir.join(REPORT_FILE);
        Self::save(report.as_bytes(), report_file)
            .await
            .with_context(|e| {
                format!(
                    "cannot create report file {}: {}",
                    self.reports_dir.join(REPORT_FILE).to_string_lossy(),
                    e
                )
            })?;

        // produce summary
        let mut summary = String::new();
        if let Some(url) = &self.reports_url {
            writeln!(
                summary,
                "Summary of the report available at {}:",
                url.join(REPORT_FILE)?
            )?;
        } else {
            writeln!(summary, "Summary of the report:")?;
        }
        let mut changed = false;
        for diff in diff {
            if let (Some(_), Some(delta)) = (diff.prev, diff.delta) {
                if delta.covered != 0 {
                    writeln!(
                        summary,
                        "{}: new edges covered since previous report (+{})",
                        diff.name, delta.covered
                    )?;
                    changed = true;
                }
            } else if let (Some(_), Some(delta)) = (diff.prev_run, diff.delta_run) {
                if (delta.covered, delta.total) != (0, 0) {
                    writeln!(
                        summary,
                        "{}: coverage changed since previous run ({}/{})",
                        diff.name, delta.covered, delta.total
                    )?;
                    changed = true;
                }
            }
        }
        if !changed {
            writeln!(summary, "No changed detected")?;
        }

        Ok(summary)
    }

    fn get_diff(
        &self,
        name: &String,
        curr: &TargetStatus,
        prev_report: &Option<FuzzingStatus>,
    ) -> TargetStatusDiff {
        let prev: Option<TargetStatus> = prev_report
            .as_ref()
            .map(|prev| prev.get(name))
            .flatten()
            .cloned();
        let prev_run: Option<TargetStatus> = self
            .previous
            .as_ref()
            .map(|prev| prev.get(name))
            .flatten()
            .cloned();
        (name.clone(), *curr, prev, prev_run).into()
    }
}
