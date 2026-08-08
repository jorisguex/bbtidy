//! Project and BitBake build-context discovery.
//!
//! BitBake projects commonly have a source tree and one or more build trees.
//! This module provides deterministic, read-only discovery for commands that
//! need a configured build context without making callers duplicate the
//! `local.conf`/`bblayers.conf` checks.

use serde::Serialize;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const BBTIDY_BUILD_DIR_ENV: &str = "BBTIDY_BITBAKE_BUILD_DIR";
const BUILDDIR_ENV: &str = "BUILDDIR";

/// How a build context was selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildContextSource {
    Explicit,
    Configured,
    BbtidyEnvironment,
    BuildDirEnvironment,
    Discovered,
}

impl BuildContextSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Configured => "configured",
            Self::BbtidyEnvironment => "bbtidy-environment",
            Self::BuildDirEnvironment => "builddir-environment",
            Self::Discovered => "discovered",
        }
    }
}

impl fmt::Display for BuildContextSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A validated BitBake build directory and its source project directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildContext {
    project_dir: PathBuf,
    build_dir: PathBuf,
    source: BuildContextSource,
}

impl BuildContext {
    /// Validates an explicitly supplied build directory.
    pub fn from_build_dir(path: impl AsRef<Path>) -> Result<Self, BuildContextError> {
        context_from_path(path.as_ref(), BuildContextSource::Explicit, None)
    }

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    pub fn build_dir(&self) -> &Path {
        &self.build_dir
    }

    pub const fn source(&self) -> BuildContextSource {
        self.source
    }
}

/// Inputs used by [`discover_build_context_with_options`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuildContextDiscoveryOptions {
    /// A path from project configuration, resolved relative to that config.
    pub configured_build_dir: Option<PathBuf>,
    /// Optional override corresponding to `BBTIDY_BITBAKE_BUILD_DIR`.
    pub bbtidy_build_dir: Option<PathBuf>,
    /// Optional override corresponding to `BUILDDIR`.
    pub build_dir_environment: Option<PathBuf>,
}

impl BuildContextDiscoveryOptions {
    pub fn from_environment() -> Self {
        Self {
            configured_build_dir: None,
            bbtidy_build_dir: env::var_os(BBTIDY_BUILD_DIR_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            build_dir_environment: env::var_os(BUILDDIR_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
        }
    }
}

/// Operational failures while discovering a project or build context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildContextError {
    InvalidStart {
        path: PathBuf,
        reason: String,
    },
    InvalidBuildDirectory {
        path: PathBuf,
        source: BuildContextSource,
        reason: String,
    },
    NotFound {
        start: PathBuf,
    },
    Ambiguous {
        start: PathBuf,
        candidates: Vec<PathBuf>,
    },
}

impl fmt::Display for BuildContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStart { path, reason } => {
                write!(
                    formatter,
                    "invalid project discovery path {}: {reason}",
                    path.display()
                )
            }
            Self::InvalidBuildDirectory {
                path,
                source,
                reason,
            } => write!(
                formatter,
                "invalid {source} BitBake build directory {}: {reason}",
                path.display()
            ),
            Self::NotFound { start } => write!(
                formatter,
                "could not discover a BitBake build directory from {}",
                start.display()
            ),
            Self::Ambiguous { start, candidates } => {
                let paths = candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    formatter,
                    "multiple BitBake build directories discovered from {}: {paths}; use --build-dir",
                    start.display()
                )
            }
        }
    }
}

impl std::error::Error for BuildContextError {}

/// Discovers a BitBake build context from the current project location and
/// the standard `BBTIDY_BITBAKE_BUILD_DIR`/`BUILDDIR` environment variables.
pub fn discover_build_context(start: impl AsRef<Path>) -> Result<BuildContext, BuildContextError> {
    discover_build_context_with_options(start, &BuildContextDiscoveryOptions::from_environment())
}

/// Discovers a BitBake build context using deterministic caller-provided
/// precedence inputs. This is useful for project configuration and tests.
pub fn discover_build_context_with_options(
    start: impl AsRef<Path>,
    options: &BuildContextDiscoveryOptions,
) -> Result<BuildContext, BuildContextError> {
    let start = normalize_start(start.as_ref())?;

    for (path, source) in [
        (
            options.configured_build_dir.as_deref(),
            BuildContextSource::Configured,
        ),
        (
            options.bbtidy_build_dir.as_deref(),
            BuildContextSource::BbtidyEnvironment,
        ),
        (
            options.build_dir_environment.as_deref(),
            BuildContextSource::BuildDirEnvironment,
        ),
    ] {
        if let Some(path) = path {
            return context_from_path(path, source, Some(&start));
        }
    }

    discover_from_ancestors(&start)
}

fn normalize_start(path: &Path) -> Result<PathBuf, BuildContextError> {
    let metadata = fs::metadata(path).map_err(|error| BuildContextError::InvalidStart {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let directory = if metadata.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    };
    fs::canonicalize(&directory).map_err(|error| BuildContextError::InvalidStart {
        path: directory,
        reason: error.to_string(),
    })
}

fn context_from_path(
    path: &Path,
    source: BuildContextSource,
    relative_to: Option<&Path>,
) -> Result<BuildContext, BuildContextError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        relative_to
            .map(Path::to_path_buf)
            .or_else(|| env::current_dir().ok())
            .unwrap_or_default()
            .join(path)
    };
    let path = fs::canonicalize(&path).unwrap_or(path);
    validate_build_directory(&path, source)?;
    let project_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| path.clone());
    Ok(BuildContext {
        project_dir,
        build_dir: path,
        source,
    })
}

fn validate_build_directory(
    path: &Path,
    source: BuildContextSource,
) -> Result<(), BuildContextError> {
    let metadata =
        fs::metadata(path).map_err(|error| BuildContextError::InvalidBuildDirectory {
            path: path.to_path_buf(),
            source,
            reason: error.to_string(),
        })?;
    if !metadata.is_dir() {
        return Err(BuildContextError::InvalidBuildDirectory {
            path: path.to_path_buf(),
            source,
            reason: "path is not a directory".to_owned(),
        });
    }
    for filename in ["conf/local.conf", "conf/bblayers.conf"] {
        if !path.join(filename).is_file() {
            return Err(BuildContextError::InvalidBuildDirectory {
                path: path.to_path_buf(),
                source,
                reason: format!("missing {filename}"),
            });
        }
    }
    Ok(())
}

fn discover_from_ancestors(start: &Path) -> Result<BuildContext, BuildContextError> {
    let mut current = Some(start);
    while let Some(directory) = current {
        if is_valid_build_directory(directory) {
            return context_from_path(directory, BuildContextSource::Discovered, None);
        }

        let mut conventional = Vec::new();
        let exact = directory.join("build");
        if is_valid_build_directory(&exact) {
            conventional.push(exact);
        }
        let entries = fs::read_dir(directory).ok();
        if let Some(entries) = entries {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("build-") && is_valid_build_directory(&path) {
                    conventional.push(path);
                }
            }
        }

        conventional.sort();
        conventional.dedup();
        if conventional.len() == 1 {
            return context_from_path(&conventional[0], BuildContextSource::Discovered, None);
        }
        if conventional.len() > 1 {
            return Err(BuildContextError::Ambiguous {
                start: start.to_path_buf(),
                candidates: conventional,
            });
        }

        current = directory.parent();
    }

    Err(BuildContextError::NotFound {
        start: start.to_path_buf(),
    })
}

fn is_valid_build_directory(path: &Path) -> bool {
    path.is_dir()
        && path.join("conf/local.conf").is_file()
        && path.join("conf/bblayers.conf").is_file()
}
