//! Resolves the one stage embedded into a desktop bundle.
//!
//! The desktop command gives the UI and host different environment names, but
//! they describe one fact. This module owns their agreement at the last build
//! boundary before either half is compiled.

use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter, Result as FormatResult},
    path::{Path, PathBuf},
};

use serde_json::Value;
use tauri_utils::config::{BuildConfig, FrontendDist};

#[derive(Debug, PartialEq, Eq)]
pub enum BuildStageError {
    Invalid { name: &'static str, value: String },
    Conflict { host: String, ui: String },
    Unknown(String),
    InvalidTauriConfig(String),
    MissingFrontendDist,
    UnsupportedFrontendDist,
    FrontendMismatch { bundle: String, frontend: String },
}

impl Display for BuildStageError {
    fn fmt(&self, out: &mut Formatter<'_>) -> FormatResult {
        match self {
            Self::Invalid { name, value } => {
                write!(
                    out,
                    "{name} must name a stage without surrounding whitespace; got {value:?}"
                )
            }
            Self::Conflict { host, ui } => write!(
                out,
                "desktop build stage conflict: NESSA_STAGE {host:?}, VITE_NESSA_STAGE {ui:?}"
            ),
            Self::Unknown(stage) => write!(out, "unknown Nessa build stage {stage:?}"),
            Self::InvalidTauriConfig(error) => {
                write!(out, "effective Tauri config is invalid: {error}")
            }
            Self::MissingFrontendDist => {
                write!(out, "effective Tauri config must name build.frontendDist")
            }
            Self::UnsupportedFrontendDist => write!(
                out,
                "effective Tauri build.frontendDist must be a directory path"
            ),
            Self::FrontendMismatch { bundle, frontend } => write!(
                out,
                "desktop bundle stage conflict: host bundle stage {bundle:?}, frontend dist stage {frontend:?}"
            ),
        }
    }
}

pub fn frontend_stage_record(
    mut config: Value,
    override_config: Option<&str>,
) -> Result<PathBuf, BuildStageError> {
    if let Some(override_config) = override_config {
        let patch = serde_json::from_str(override_config)
            .map_err(|error| BuildStageError::InvalidTauriConfig(error.to_string()))?;
        json_patch::merge(&mut config, &patch);
    }
    let build = config
        .pointer("/build/frontendDist")
        .map(|_| config["build"].clone())
        .ok_or(BuildStageError::MissingFrontendDist)?;
    let build: BuildConfig = serde_json::from_value(build)
        .map_err(|error| BuildStageError::InvalidTauriConfig(error.to_string()))?;
    match build.frontend_dist {
        Some(FrontendDist::Directory(directory)) => Ok(directory.join("nessa-stage.json")),
        Some(FrontendDist::Url(_) | FrontendDist::Files(_)) => {
            Err(BuildStageError::UnsupportedFrontendDist)
        }
        _ => Err(BuildStageError::MissingFrontendDist),
    }
}

pub fn frontend_index(record: &Path) -> Option<PathBuf> {
    record
        .parent()
        .map(|directory| directory.join("index.html"))
}

pub fn verify_frontend(
    known: &BTreeSet<String>,
    bundle: &str,
    frontend: &str,
) -> Result<(), BuildStageError> {
    if !known.contains(frontend) {
        return Err(BuildStageError::Unknown(frontend.to_owned()));
    }
    if frontend != bundle {
        return Err(BuildStageError::FrontendMismatch {
            bundle: bundle.to_owned(),
            frontend: frontend.to_owned(),
        });
    }
    Ok(())
}

fn explicit(name: &'static str, value: Option<&str>) -> Result<Option<String>, BuildStageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() || value.trim() != value {
        return Err(BuildStageError::Invalid {
            name,
            value: value.to_owned(),
        });
    }
    Ok(Some(value.to_owned()))
}

pub fn resolve(
    known: &BTreeSet<String>,
    host: Option<&str>,
    ui: Option<&str>,
    fallback: &str,
) -> Result<String, BuildStageError> {
    let host = explicit("NESSA_STAGE", host)?;
    let ui = explicit("VITE_NESSA_STAGE", ui)?;
    if let (Some(host), Some(ui)) = (&host, &ui) {
        if host != ui {
            return Err(BuildStageError::Conflict {
                host: host.clone(),
                ui: ui.clone(),
            });
        }
    }
    let stage = host.or(ui).unwrap_or_else(|| fallback.to_owned());
    if !known.contains(&stage) {
        return Err(BuildStageError::Unknown(stage));
    }
    Ok(stage)
}
