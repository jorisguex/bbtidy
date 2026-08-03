use crate::{FormatOptions, LintOptions, LintSeverity, MetadataListLayout, lint_rules};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_FILE_NAME: &str = ".bbtidy.toml";

/// Resolved bbtidy configuration for one CLI invocation.
#[derive(Clone, Debug)]
pub struct Config {
    pub format: FormatOptions,
    pub lint: LintOptions,
    base_dir: PathBuf,
    excludes: GlobSet,
}

impl Config {
    fn default_for(base_dir: PathBuf) -> Self {
        Self {
            format: FormatOptions::default(),
            lint: LintOptions::default(),
            base_dir,
            excludes: empty_glob_set(),
        }
    }

    /// Returns whether a path matches one of the configured exclusion globs.
    pub fn is_excluded(&self, path: &Path) -> bool {
        if self.excludes.is_empty() {
            return false;
        }

        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let Ok(current_dir) = env::current_dir() else {
                return false;
            };
            current_dir.join(path)
        };
        let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
        let Ok(relative) = absolute.strip_prefix(&self.base_dir) else {
            return false;
        };
        self.excludes.is_match(relative)
    }

    fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let path = fs::canonicalize(path).map_err(|error| {
            ConfigError::new(format!(
                "could not locate config {}: {error}",
                path.display()
            ))
        })?;
        let source = fs::read_to_string(&path).map_err(|error| {
            ConfigError::new(format!("could not read config {}: {error}", path.display()))
        })?;
        let file_config: FileConfig = toml::from_str(&source).map_err(|error| {
            ConfigError::new(format!(
                "could not parse config {}: {error}",
                path.display()
            ))
        })?;
        Self::from_file_config(file_config, path.parent().unwrap_or(Path::new(".")))
    }

    fn from_file_config(file_config: FileConfig, base_dir: &Path) -> Result<Self, ConfigError> {
        let known_rules: BTreeSet<&str> = lint_rules().iter().map(|rule| rule.id()).collect();
        let disabled_rules: BTreeSet<String> = file_config
            .lint
            .disable
            .into_iter()
            .map(|rule_id| validate_rule_id(&known_rules, &rule_id).map(|()| rule_id))
            .collect::<Result<_, _>>()?;

        let mut severity_overrides = BTreeMap::new();
        for (rule_id, severity) in file_config.lint.severity {
            validate_rule_id(&known_rules, &rule_id)?;
            let severity = severity.parse::<LintSeverity>().map_err(ConfigError::new)?;
            severity_overrides.insert(rule_id, severity);
        }

        let mut glob_builder = GlobSetBuilder::new();
        for pattern in file_config.paths.exclude {
            if pattern.is_empty() {
                return Err(ConfigError::new(
                    "path exclusion patterns must not be empty",
                ));
            }
            let glob = Glob::new(&pattern).map_err(|error| {
                ConfigError::new(format!("invalid path exclusion '{pattern}': {error}"))
            })?;
            glob_builder.add(glob);
        }
        let excludes = glob_builder.build().map_err(|error| {
            ConfigError::new(format!("could not build path exclusions: {error}"))
        })?;

        Ok(Self {
            format: FormatOptions {
                max_top_level_blank_lines: file_config
                    .format
                    .max_top_level_blank_lines
                    .unwrap_or(FormatOptions::default().max_top_level_blank_lines),
                metadata_list_layout: file_config
                    .format
                    .metadata_list_layout
                    .unwrap_or(FormatOptions::default().metadata_list_layout),
            },
            lint: LintOptions::from_parts(disabled_rules, severity_overrides),
            base_dir: base_dir.to_path_buf(),
            excludes,
        })
    }
}

/// Loads an explicit config, the nearest auto-discovered config, or defaults.
pub fn load_config(explicit: Option<&Path>, no_config: bool) -> Result<Config, ConfigError> {
    let current_dir = env::current_dir().map_err(|error| {
        ConfigError::new(format!("could not determine current directory: {error}"))
    })?;
    let base_dir = fs::canonicalize(&current_dir).map_err(|error| {
        ConfigError::new(format!("could not resolve current directory: {error}"))
    })?;

    if no_config {
        return Ok(Config::default_for(base_dir));
    }

    let path = explicit
        .map(PathBuf::from)
        .or_else(|| discover_config(&current_dir));
    match path {
        Some(path) => Config::from_file(&path),
        None => Ok(Config::default_for(base_dir)),
    }
}

/// Finds the nearest `.bbtidy.toml` at or above a directory.
pub fn discover_config(start: &Path) -> Option<PathBuf> {
    let mut directory = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };

    loop {
        let candidate = directory.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !directory.pop() {
            return None;
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    format: FileFormatConfig,
    #[serde(default)]
    lint: FileLintConfig,
    #[serde(default)]
    paths: FilePathsConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFormatConfig {
    max_top_level_blank_lines: Option<usize>,
    metadata_list_layout: Option<MetadataListLayout>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLintConfig {
    #[serde(default)]
    disable: Vec<String>,
    #[serde(default)]
    severity: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePathsConfig {
    #[serde(default)]
    exclude: Vec<String>,
}

fn validate_rule_id(known_rules: &BTreeSet<&str>, rule_id: &str) -> Result<(), ConfigError> {
    if known_rules.contains(rule_id) {
        Ok(())
    } else {
        Err(ConfigError::new(format!(
            "unknown lint rule '{rule_id}'; use one of {}",
            known_rules.iter().copied().collect::<Vec<_>>().join(", ")
        )))
    }
}

fn empty_glob_set() -> GlobSet {
    GlobSetBuilder::new()
        .build()
        .expect("an empty glob set must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(source: &str) -> Config {
        let file_config: FileConfig = toml::from_str(source).unwrap();
        Config::from_file_config(file_config, Path::new("/project")).unwrap()
    }

    #[test]
    fn defaults_are_preserved_without_settings() {
        let config = config("");

        assert_eq!(config.format, FormatOptions::default());
        assert_eq!(config.lint, LintOptions::default());
        assert!(!config.is_excluded(Path::new("/project/recipes/example.bb")));
    }

    #[test]
    fn parses_format_lint_and_path_settings() {
        let config = config(
            r#"
[format]
max_top_level_blank_lines = 0
metadata_list_layout = "one-per-line"

[lint]
disable = ["BBT003", "BBT010"]

[lint.severity]
BBT001 = "error"
BBT010 = "info"

[paths]
exclude = ["vendor/**", "**/files/**"]
"#,
        );

        assert_eq!(config.format.max_top_level_blank_lines, 0);
        assert_eq!(
            config.format.metadata_list_layout,
            MetadataListLayout::OnePerLine
        );
        assert!(config.is_excluded(Path::new("/project/vendor/example.bb")));
        assert!(config.is_excluded(Path::new("/project/recipes/example/files/data.inc")));
        assert!(!config.is_excluded(Path::new("/project/recipes/example.bb")));
    }

    #[test]
    fn rejects_unknown_rules_and_invalid_severities() {
        let unknown_rule: FileConfig = toml::from_str(
            r#"
[lint]
disable = ["BBT999"]
"#,
        )
        .unwrap();
        let error = Config::from_file_config(unknown_rule, Path::new("/project")).unwrap_err();
        assert!(error.to_string().contains("unknown lint rule 'BBT999'"));

        let invalid_severity: FileConfig = toml::from_str(
            r#"
[lint.severity]
BBT001 = "fatal"
"#,
        )
        .unwrap();
        let error = Config::from_file_config(invalid_severity, Path::new("/project")).unwrap_err();
        assert!(error.to_string().contains("invalid lint severity 'fatal'"));

        let invalid_glob: FileConfig = toml::from_str(
            r#"
[paths]
exclude = ["["]
"#,
        )
        .unwrap();
        let error = Config::from_file_config(invalid_glob, Path::new("/project")).unwrap_err();
        assert!(error.to_string().contains("invalid path exclusion '['"));
    }

    #[test]
    fn discovers_the_nearest_config() {
        let directory = std::env::temp_dir().join(format!(
            "bbtidy-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = directory.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(directory.join(CONFIG_FILE_NAME), "").unwrap();
        assert_eq!(
            discover_config(&nested),
            Some(directory.join(CONFIG_FILE_NAME))
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
