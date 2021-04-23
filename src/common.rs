use std::{ffi::{OsStr, OsString}, path::{Path, PathBuf}};

use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
use url::Url;

use crate::error::Error;

pub fn new_local_path(segments: &[&str]) -> PathBuf {
    segments.iter().map(|s| sanitize_path_segment(s)).collect()
}

/// Sanitize path segment (directory/file) by replacing invalid characters with underscores
pub fn sanitize_path_segment(segment: &str) -> OsString {
    let sanitize_options = sanitize_filename::Options {
        replacement: "_",
        ..Default::default()
    };
    sanitize_filename::sanitize_with_options(segment, sanitize_options).into()
}

/// Append relative fs path to the Url.
pub fn reports_url(reports_url: &Url, rel_path: &Path) -> Result<Url, Error> {
    //    assert!(rel_path.)
    let mut reports_url = reports_url.clone();
    for segment in rel_path {
        reports_url = reports_url.join(&sanitize_url_path_segment(segment))?
    }
    Ok(reports_url)
}

pub fn sanitize_url_path_segment(segment: &OsStr) -> String {
    percent_encode(
        segment.to_string_lossy().as_ref().as_bytes(),
        NON_ALPHANUMERIC,
    )
    .to_string()
}
