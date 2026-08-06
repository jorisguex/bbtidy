use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const UNFORMATTED: &str = "SUMMARY=\"demo\"\n";
const FORMATTED: &str = "SUMMARY = \"demo\"\n";

#[test]
fn syntax_stats_reports_machine_readable_cst_coverage() {
    let directory = TemporaryDirectory::new("syntax-stats");
    let file = directory.write(
        "example.bb",
        "SUMMARY=\"demo\"\n# note\n\nunsupported statement\n",
    );

    let output = run(["syntax-stats", file.to_str().unwrap()]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["files"], 1);
    assert_eq!(report["total_nodes"], 4);
    assert_eq!(report["structured_nodes"], 1);
    assert_eq!(report["trivia_nodes"], 2);
    assert_eq!(report["unknown_nodes"], 1);
    assert_eq!(report["unknown_bytes"], 22);
}

#[test]
fn format_prints_to_stdout_without_modifying_the_file() {
    let directory = TemporaryDirectory::new("format-stdout");
    let file = directory.write("example.bb", UNFORMATTED);

    let output = run(["format", file.to_str().unwrap()]);

    assert_success(&output);
    assert_eq!(output.stdout, FORMATTED.as_bytes());
    assert_eq!(fs::read_to_string(file).unwrap(), UNFORMATTED);
}

#[test]
fn format_reads_from_standard_input() {
    let output = run_with_stdin(["format", "-"], UNFORMATTED);

    assert_success(&output);
    assert_eq!(output.stdout, FORMATTED.as_bytes());
}

#[test]
fn format_diff_emits_a_unified_diff_without_modifying_the_file() {
    let directory = TemporaryDirectory::new("format-diff");
    let file = directory.write("example.bb", UNFORMATTED);

    let output = run(["format", "--diff", file.to_str().unwrap()]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!("--- a/{}", file.display())));
    assert!(stdout.contains(&format!("+++ b/{}", file.display())));
    assert!(stdout.contains("-SUMMARY=\"demo\""));
    assert!(stdout.contains("+SUMMARY = \"demo\""));
    assert_eq!(fs::read_to_string(file).unwrap(), UNFORMATTED);
}

#[test]
fn check_and_write_have_stable_exit_codes() {
    let directory = TemporaryDirectory::new("check-write");
    let file = directory.write("example.bb", UNFORMATTED);
    let path = file.to_str().unwrap();

    let check_before = run(["check", path]);
    assert_eq!(check_before.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(check_before.stdout).unwrap(),
        format!("would reformat: {}\n", file.display())
    );

    let write = run(["format", "--write", path]);
    assert_success(&write);
    assert_eq!(fs::read_to_string(&file).unwrap(), FORMATTED);

    let check_after = run(["check", path]);
    assert_success(&check_after);
    assert!(check_after.stdout.is_empty());
}

#[test]
fn config_controls_formatter_blank_lines() {
    let directory = TemporaryDirectory::new("config-format");
    let config = directory.write(".bbtidy.toml", "[format]\nmax_top_level_blank_lines = 0\n");
    let file = directory.write("example.bb", "A=\"a\"\n\n\nB=\"b\"\n");

    let output = run([
        "format",
        "--config",
        config.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "A = \"a\"\nB = \"b\"\n"
    );
}

#[test]
fn config_enables_conservative_metadata_list_layout() {
    let directory = TemporaryDirectory::new("config-metadata-list-layout");
    let config = directory.write(
        ".bbtidy.toml",
        "[format]\nmetadata_list_layout = \"one-per-line\"\n",
    );
    let file = directory.write(
        "example.bb",
        concat!(
            "SRC_URI= \" \\\n",
            "\tfile://one.patch \\\n",
            " file://two.patch \\\n",
            "\"\n",
        ),
    );

    let output = run([
        "format",
        "--config",
        config.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "SRC_URI = \" \\\n",
            "    file://one.patch \\\n",
            "    file://two.patch \\\n",
            "    \"\n",
        )
    );
}

#[test]
fn config_filters_lint_rules_and_overrides_severity() {
    let directory = TemporaryDirectory::new("config-lint");
    let config = directory.write(
        ".bbtidy.toml",
        concat!(
            "[lint]\n",
            "disable = [\"BBT001\", \"BBT004\"]\n",
            "\n",
            "[lint.severity]\n",
            "BBT005 = \"error\"\n",
        ),
    );
    let file = directory.write(
        "example.bb",
        concat!(
            "SUMMARY = \"demo\"  \n",
            "SRCREV = \"${AUTOREV}\"\n",
            "inherit cmake\n",
            "inherit cmake\n",
        ),
    );

    let output = run([
        "lint",
        "--config",
        config.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "{}:4:9: error[BBT005]: class 'cmake' is inherited more than once\n",
            file.display()
        )
    );
}

#[test]
fn config_excludes_paths_from_directory_processing() {
    let directory = TemporaryDirectory::new("config-exclude");
    let config = directory.write(".bbtidy.toml", "[paths]\nexclude = [\"ignored/**\"]\n");
    let kept = directory.write("kept.bb", UNFORMATTED);
    let ignored = directory.write("ignored/example.bb", UNFORMATTED);

    let output = run([
        "format",
        "--write",
        "--config",
        config.to_str().unwrap(),
        directory.path().to_str().unwrap(),
    ]);

    assert_success(&output);
    assert_eq!(fs::read_to_string(&kept).unwrap(), FORMATTED);
    assert_eq!(fs::read_to_string(&ignored).unwrap(), UNFORMATTED);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&kept.display().to_string()));
    assert!(!stdout.contains("ignored/example.bb"));
}

#[test]
fn format_write_enforces_repository_file_limit_before_rewriting() {
    let directory = TemporaryDirectory::new("safety-file-limit");
    let config = directory.write(".bbtidy.toml", "[safety]\nmax_files = 1\n");
    let first = directory.write("first.bb", UNFORMATTED);
    let second = directory.write("second.bb", UNFORMATTED);

    let output = run([
        "format",
        "--write",
        "--config",
        config.to_str().unwrap(),
        directory.path().to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("safety limit exceeded")
    );
    assert_eq!(fs::read_to_string(first).unwrap(), UNFORMATTED);
    assert_eq!(fs::read_to_string(second).unwrap(), UNFORMATTED);
}

#[test]
fn format_write_enforces_repository_byte_limit_before_rewriting() {
    let directory = TemporaryDirectory::new("safety-byte-limit");
    let config = directory.write(".bbtidy.toml", "[safety]\nmax_bytes = 4\n");
    let file = directory.write("example.bb", UNFORMATTED);

    let output = run([
        "format",
        "--write",
        "--config",
        config.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("safety limit exceeded")
    );
    assert_eq!(fs::read_to_string(file).unwrap(), UNFORMATTED);
}

#[cfg(unix)]
#[test]
fn format_write_rejects_symlinks_before_changing_any_file() {
    use std::os::unix::fs::symlink;

    let directory = TemporaryDirectory::new("safety-symlink");
    let first = directory.write("a.bb", UNFORMATTED);
    let target = directory.write("target.bb", UNFORMATTED);
    let link = directory.path().join("z.bb");
    symlink(&target, &link).unwrap();

    let output = run([
        "format",
        "--write",
        first.to_str().unwrap(),
        link.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("symbolic link")
    );
    assert_eq!(fs::read_to_string(&first).unwrap(), UNFORMATTED);
    assert_eq!(fs::read_to_string(&target).unwrap(), UNFORMATTED);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn invalid_config_is_an_operational_error() {
    let directory = TemporaryDirectory::new("config-invalid");
    let config = directory.write(".bbtidy.toml", "[lint]\ndisable = [\"BBT999\"]\n");
    let file = directory.write("example.bb", UNFORMATTED);

    let output = run([
        "check",
        "--config",
        config.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("unknown lint rule 'BBT999'")
    );
}

#[test]
fn directories_are_recursive_filtered_and_deterministic() {
    let directory = TemporaryDirectory::new("directory");
    let nested = directory.write("nested/a.bb", UNFORMATTED);
    let root = directory.write("z.bb", UNFORMATTED);
    let ignored = directory.write("notes.txt", UNFORMATTED);

    let output = run(["format", "--write", directory.path().to_str().unwrap()]);

    assert_success(&output);
    assert_eq!(fs::read_to_string(&nested).unwrap(), FORMATTED);
    assert_eq!(fs::read_to_string(&root).unwrap(), FORMATTED);
    assert_eq!(fs::read_to_string(&ignored).unwrap(), UNFORMATTED);

    let stdout = String::from_utf8(output.stdout).unwrap();
    let nested_position = stdout.find(&nested.display().to_string()).unwrap();
    let root_position = stdout.find(&root.display().to_string()).unwrap();
    assert!(
        nested_position < root_position,
        "directory output was not sorted: {stdout}"
    );
    assert!(!stdout.contains(&ignored.display().to_string()));
}

#[test]
fn directory_discovery_skips_recipe_payload_files() {
    let directory = TemporaryDirectory::new("payload-files");
    let layer_configuration = directory.write("conf/layer.conf", UNFORMATTED);
    let metadata_include = directory.write("recipes-example/example/example.inc", UNFORMATTED);
    let payload_configuration =
        directory.write("recipes-example/example/example/runtime.conf", UNFORMATTED);
    let files_include = directory.write("recipes-example/example/files/template.inc", UNFORMATTED);

    let output = run(["format", "--write", directory.path().to_str().unwrap()]);

    assert_success(&output);
    assert_eq!(fs::read_to_string(layer_configuration).unwrap(), FORMATTED);
    assert_eq!(fs::read_to_string(metadata_include).unwrap(), FORMATTED);
    assert_eq!(
        fs::read_to_string(payload_configuration).unwrap(),
        UNFORMATTED
    );
    assert_eq!(fs::read_to_string(files_include).unwrap(), UNFORMATTED);
}

#[test]
fn a_batch_format_error_prevents_all_writes() {
    let directory = TemporaryDirectory::new("batch-error");
    let valid = directory.write("valid.bb", UNFORMATTED);
    let malformed = directory.write("malformed.bb", "BROKEN = \"value\n");

    let output = run(["format", "--write", directory.path().to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(valid).unwrap(), UNFORMATTED);
    assert_eq!(fs::read_to_string(malformed).unwrap(), "BROKEN = \"value\n");
}

#[test]
fn format_stdout_rejects_multiple_inputs() {
    let directory = TemporaryDirectory::new("multiple-stdout");
    let first = directory.write("first.bb", FORMATTED);
    let second = directory.write("second.bb", FORMATTED);

    let output = run(["format", first.to_str().unwrap(), second.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("requires exactly one input")
    );
}

#[test]
fn lex_accepts_standard_input_and_reports_lexer_errors() {
    let valid = run_with_stdin(["lex", "-"], FORMATTED);
    assert_success(&valid);
    assert!(String::from_utf8(valid.stdout).unwrap().contains("Ident"));

    let invalid = run_with_stdin(["lex", "-"], "@");
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8(invalid.stdout).unwrap().contains("Error"));
}

#[test]
fn lint_reports_source_ordered_findings_and_stable_exit_codes() {
    let directory = TemporaryDirectory::new("lint");
    let file = directory.write(
        "example.bb",
        concat!(
            "SUMMARY = \"demo\"  \n",
            "SRCREV = \"${AUTOREV}\"\n",
            "inherit cmake\n",
            "inherit cmake\n",
        ),
    );

    let output = run(["lint", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let findings = stdout.lines().collect::<Vec<_>>();
    assert_eq!(findings.len(), 3);
    assert!(findings[0].contains(":1:17: warning[BBT001]:"));
    assert!(findings[1].contains(":2:11: warning[BBT004]:"));
    assert!(findings[2].contains(":4:9: warning[BBT005]:"));
    assert!(output.stderr.is_empty());

    fs::write(&file, "SUMMARY = \"demo\"\n").unwrap();
    let clean = run(["lint", file.to_str().unwrap()]);
    assert_success(&clean);
    assert!(clean.stdout.is_empty());
}

#[test]
fn lint_json_output_has_a_stable_schema() {
    let directory = TemporaryDirectory::new("lint-json");
    let file = directory.write(
        "example.bb",
        concat!(
            "SUMMARY = \"demo\"  \n",
            "SRCREV = \"${AUTOREV}\"\n",
            "inherit cmake\n",
            "inherit cmake\n",
        ),
    );

    let output = run(["lint", "--output", "json", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 3);
    assert_eq!(diagnostics[0]["path"], file.display().to_string());
    assert_eq!(diagnostics[0]["line"], 1);
    assert_eq!(diagnostics[0]["column"], 17);
    assert_eq!(diagnostics[0]["severity"], "warning");
    assert_eq!(diagnostics[0]["rule_id"], "BBT001");
}

#[test]
fn lint_sarif_output_contains_rules_locations_and_results() {
    let directory = TemporaryDirectory::new("lint-sarif");
    let file = directory.write("example.bb", "SRCREV = \"${AUTOREV}\"\n");

    let output = run(["lint", "--output", "sarif", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], "2.1.0");
    assert_eq!(
        report["$schema"],
        "https://json.schemastore.org/sarif-2.1.0.json"
    );
    let run = &report["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "bbtidy");
    assert_eq!(run["tool"]["driver"]["rules"].as_array().unwrap().len(), 10);
    let result = &run["results"][0];
    assert_eq!(result["ruleId"], "BBT004");
    assert_eq!(result["level"], "warning");
    assert_eq!(
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        file.display().to_string()
    );
    assert_eq!(
        result["locations"][0]["physicalLocation"]["region"]["startLine"],
        1
    );
}

#[test]
fn workspace_cycles_are_reported_in_json_and_sarif() {
    let directory = TemporaryDirectory::new("lint-workspace-cycle");
    directory.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
    directory.write("recipes-example/example/helper.inc", "require example.bb\n");
    let recipe = directory.write("recipes-example/example/example.bb", "require helper.inc\n");

    let json = run([
        "lint",
        "--output",
        "json",
        directory.path().to_str().unwrap(),
    ]);
    assert_eq!(json.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["rule_id"] == "BBT010")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["path"] == recipe.display().to_string())
    );

    let sarif = run([
        "lint",
        "--output",
        "sarif",
        directory.path().to_str().unwrap(),
    ]);
    assert_eq!(sarif.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&sarif.stdout).unwrap();
    let run = &report["runs"][0];
    assert!(
        run["tool"]["driver"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["id"] == "BBT010")
    );
    assert!(
        run["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["ruleId"] == "BBT010")
    );
}

#[test]
fn machine_lint_output_is_not_partial_when_analysis_fails() {
    let directory = TemporaryDirectory::new("lint-json-error");
    let file = directory.write("malformed.bb", "SUMMARY = \"unterminated\n");

    let output = run(["lint", "--output", "json", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("top-level assignment contains an unclosed quote")
    );
}

#[test]
fn lint_directories_report_unresolved_static_layer_references() {
    let directory = TemporaryDirectory::new("lint-semantic");
    directory.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
    directory.write("classes/base.bbclass", "BASE = \"1\"\n");
    directory.write("recipes-example/common.inc", "COMMON = \"1\"\n");
    let recipe = directory.write(
        "recipes-example/example.bb",
        concat!(
            "inherit base missing\n",
            "require common.inc\n",
            "require missing.inc\n",
            "inherit ${DYNAMIC}\n",
            "require ${DYNAMIC}\n",
        ),
    );

    let output = run(["lint", directory.path().to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stderr, b"");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let findings = stdout.lines().collect::<Vec<_>>();
    assert_eq!(findings.len(), 2);
    assert!(findings[0].contains(":1:14: warning[BBT007]:"));
    assert!(findings[0].contains("inherited class 'missing'"));
    assert!(findings[1].contains(":3:9: warning[BBT006]:"));
    assert!(findings[1].contains("required file 'missing.inc'"));
    assert!(stdout.contains(&recipe.display().to_string()));
}

#[test]
fn lint_accepts_standard_input() {
    let output = run_with_stdin(["lint", "-"], "SUMMARY = \"demo\"");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "<stdin>:1:17: warning[BBT002]: file does not end with a newline\n"
    );
}

#[test]
fn lint_directories_are_deterministic_and_malformed_input_is_an_error() {
    let directory = TemporaryDirectory::new("lint-directory");
    let nested = directory.write("nested/a.bb", "A = \"a\"");
    let root = directory.write("z.bb", "Z = \"z\"");
    directory.write("ignored.txt", "IGNORED = \"value\"");

    let output = run(["lint", directory.path().to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.find(&nested.display().to_string()).unwrap()
            < stdout.find(&root.display().to_string()).unwrap()
    );
    assert!(!stdout.contains("ignored.txt"));

    fs::write(&nested, "BROKEN = \"value\n").unwrap();
    let malformed = run(["lint", directory.path().to_str().unwrap()]);
    assert_eq!(malformed.status.code(), Some(2));
    assert!(
        String::from_utf8(malformed.stderr)
            .unwrap()
            .contains("top-level assignment contains an unclosed quote")
    );
}

fn run<const N: usize>(arguments: [&str; N]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bbtidy"))
        .args(arguments)
        .output()
        .unwrap()
}

fn run_with_stdin<const N: usize>(arguments: [&str; N], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bbtidy"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "bbtidy-cli-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, relative_path: &str, contents: &str) -> PathBuf {
        let path = self.path.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
