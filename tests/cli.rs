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
fn config_wraps_static_single_line_lists_and_directives() {
    let directory = TemporaryDirectory::new("config-single-line-layout");
    let config = directory.write(
        ".bbtidy.toml",
        "[format]\nmetadata_list_layout = \"one-per-line\"\n",
    );
    let file = directory.write(
        "example.bb",
        "DEPENDS=\"build-a build-b\"\ninherit autotools pkgconfig\n",
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
            "DEPENDS = \" \\\n",
            "    build-a \\\n",
            "    build-b \\\n",
            "    \"\n",
            "inherit \\\n",
            "    autotools \\\n",
            "    pkgconfig\n",
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
fn lint_failure_threshold_controls_status_without_filtering_findings() {
    let directory = TemporaryDirectory::new("lint-failure-threshold");
    let file = directory.write("example.bb", "SRCREV = \"${AUTOREV}\"\n");
    let path = file.to_str().unwrap();

    let default = run(["lint", path]);
    assert_eq!(default.status.code(), Some(1));

    let error_threshold = run(["lint", "--fail-on", "error", path]);
    assert_eq!(error_threshold.status.code(), Some(0));
    assert!(
        String::from_utf8(error_threshold.stdout)
            .unwrap()
            .contains("warning[BBT004]")
    );

    let info_threshold = run(["lint", "--fail-on", "info", path]);
    assert_eq!(info_threshold.status.code(), Some(1));

    let never_threshold = run(["lint", "--fail-on", "never", path]);
    assert_eq!(never_threshold.status.code(), Some(0));

    let json = run(["lint", "--output", "json", "--fail-on", "error", path]);
    assert_eq!(json.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 1);
}

#[test]
fn lint_failure_threshold_cli_overrides_config() {
    let directory = TemporaryDirectory::new("lint-failure-threshold-config");
    let config = directory.write(".bbtidy.toml", "[lint]\nfail_on = \"never\"\n");
    let file = directory.write("example.bb", "SRCREV = \"${AUTOREV}\"\n");
    let path = file.to_str().unwrap();
    let config_path = config.to_str().unwrap();

    let from_config = run(["lint", "--config", config_path, path]);
    assert_eq!(from_config.status.code(), Some(0));

    let from_cli = run(["lint", "--config", config_path, "--fail-on", "info", path]);
    assert_eq!(from_cli.status.code(), Some(1));
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
    assert_eq!(diagnostics[0]["end_line"], 1);
    assert_eq!(diagnostics[0]["end_column"], 19);
    assert_eq!(diagnostics[0]["fixable"], true);
    assert_eq!(diagnostics[0]["fixes"][0]["replacement"], "");
}

#[test]
fn lint_show_fixes_explains_safe_edits_without_changing_default_text() {
    let directory = TemporaryDirectory::new("lint-show-fixes");
    let file = directory.write("example.bb", "SUMMARY = \"demo\"  \n");

    let output = run(["lint", "--show-fixes", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("warning[BBT001]: line ends with whitespace"));
    assert!(stdout.contains("help: Remove the trailing spaces or tabs from this line."));
    assert!(stdout.contains("fix: remove trailing whitespace (bytes 16..18)"));
    assert_eq!(fs::read_to_string(file).unwrap(), "SUMMARY = \"demo\"  \n");
}

#[test]
fn lint_fix_applies_safe_edits_and_reports_remaining_findings() {
    let directory = TemporaryDirectory::new("lint-fix");
    let file = directory.write(
        "example.bb",
        "SUMMARY = \"demo\"  \nSRCREV = \"${AUTOREV}\"",
    );

    let output = run(["lint", "--fix", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!("fixed: {} (2 edits)", file.display())));
    assert!(stdout.contains("warning[BBT004]:"));
    assert!(!stdout.contains("BBT001"));
    assert!(!stdout.contains("BBT002"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "SUMMARY = \"demo\"\nSRCREV = \"${AUTOREV}\"\n"
    );

    let second = run(["lint", "--fix", file.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(1));
    assert!(!String::from_utf8(second.stdout).unwrap().contains("fixed:"));
}

#[test]
fn lint_fix_json_reports_applied_edits_and_empty_post_fix_findings() {
    let directory = TemporaryDirectory::new("lint-fix-json");
    let file = directory.write("example.bb", "SUMMARY = \"demo\"  ");

    let output = run(["lint", "--fix", "--output", "json", file.to_str().unwrap()]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 0);
    assert_eq!(
        report["fixes_applied"][0]["path"],
        file.display().to_string()
    );
    assert_eq!(report["fixes_applied"][0]["count"], 2);
    assert_eq!(fs::read_to_string(file).unwrap(), "SUMMARY = \"demo\"\n");
}

#[test]
fn lint_fix_rejects_standard_input_before_reading_or_writing() {
    let output = run_with_stdin(["lint", "--fix", "-"], "SUMMARY = \"demo\"");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--fix cannot be used with standard input")
    );
}

#[test]
fn lint_fix_is_transactional_when_another_input_fails_analysis() {
    let directory = TemporaryDirectory::new("lint-fix-transaction");
    let fixable = directory.write("a.bb", "SUMMARY = \"demo\"  ");
    directory.write("b.bb", "BROKEN = \"unterminated\n");

    let output = run(["lint", "--fix", directory.path().to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(fixable).unwrap(), "SUMMARY = \"demo\"  ");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("could not lint")
    );
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
    assert_eq!(run["tool"]["driver"]["rules"].as_array().unwrap().len(), 37);
    assert_eq!(
        run["tool"]["driver"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["id"] == "BBT001")
            .unwrap()["properties"]["fixable"],
        true
    );
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
fn lint_sarif_output_contains_formal_fix_metadata() {
    let directory = TemporaryDirectory::new("lint-sarif-fix");
    let file = directory.write("example.bb", "SUMMARY = \"demo\"  \n");

    let output = run(["lint", "--output", "sarif", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let result = &report["runs"][0]["results"][0];
    assert_eq!(result["ruleId"], "BBT001");
    assert_eq!(result["properties"]["fixable"], true);
    assert_eq!(
        result["fixes"][0]["description"]["text"],
        "remove trailing whitespace"
    );
    assert_eq!(
        result["fixes"][0]["artifactChanges"][0]["replacements"][0]["insertedContent"]["text"],
        ""
    );
    assert!(result["properties"].get("help").is_some());
}

#[test]
fn workspace_cycles_are_reported_in_json_and_sarif() {
    let directory = TemporaryDirectory::new("lint-workspace-cycle");
    directory.write(
        "conf/layer.conf",
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "BBFILE_COLLECTIONS += \"test\"\n",
            "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_test = \"1\"\n",
            "LAYERSERIES_COMPAT_test = \"test\"\n",
        ),
    );
    directory.write("recipes-example/example/helper.inc", "require example.bb\n");
    let recipe = directory.write(
        "recipes-example/example/example.bb",
        "require helper.inc\nSUMMARY = \"example\"\nDESCRIPTION = \"example\"\nLICENSE = \"CLOSED\"\n",
    );

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
    directory.write(
        "conf/layer.conf",
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "BBFILE_COLLECTIONS += \"test\"\n",
            "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_test = \"1\"\n",
            "LAYERSERIES_COMPAT_test = \"test\"\n",
        ),
    );
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
            "SUMMARY = \"example\"\n",
            "DESCRIPTION = \"example\"\n",
            "LICENSE = \"CLOSED\"\n",
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
fn lint_reports_broader_recipe_metadata_rules_in_machine_output() {
    let directory = TemporaryDirectory::new("lint-recipe-metadata");
    directory.write(
        "conf/layer.conf",
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "BBFILE_COLLECTIONS += \"test\"\n",
            "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_test = \"1\"\n",
            "LAYERSERIES_COMPAT_test = \"test\"\n",
        ),
    );
    let recipe = directory.write("recipes-example/example.bb", "SUMMARY = \"example\"\n");

    let output = run([
        "lint",
        "--output",
        "json",
        recipe.to_str().unwrap(),
        directory.path().join("conf/layer.conf").to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic["rule_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["BBT012", "BBT013"]
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| { diagnostic["path"] == recipe.display().to_string() })
    );
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

#[cfg(unix)]
#[test]
fn semantic_command_runs_bitbake_and_emits_resolved_json() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TemporaryDirectory::new("semantic-cli");
    let build_dir = directory.write("build/conf/local.conf", "MACHINE = \"qemux86-64\"\n");
    let build_dir = build_dir.parent().unwrap().parent().unwrap().to_path_buf();
    directory.write("build/conf/bblayers.conf", "BBLAYERS = \"/layer\"\n");
    let bitbake = directory.write(
        "fake-bitbake",
        r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--environment" ]; then
  echo 'PN="demo"'
  exit 0
fi
exit 0
"###,
    );
    let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bitbake, permissions).unwrap();

    let output = run([
        "semantic",
        "--build-dir",
        build_dir.to_str().unwrap(),
        "--bitbake",
        bitbake.to_str().unwrap(),
        "--target",
        "demo",
        "--variable",
        "PN",
        "--output",
        "json",
    ]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["parse_succeeded"], true);
    assert_eq!(report["analysis_succeeded"], true);
    assert_eq!(report["environments"][0]["target"], "demo");
    assert_eq!(report["environments"][0]["variables"]["PN"], "demo");
}

#[cfg(unix)]
#[test]
fn semantic_lint_integrates_bitbake_diagnostics_and_resolved_metadata() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TemporaryDirectory::new("lint-bitbake-semantic");
    directory.write("build/conf/local.conf", "MACHINE = \"qemux86-64\"\n");
    directory.write("build/conf/bblayers.conf", "BBLAYERS = \"/layer\"\n");
    let recipe = directory.write("recipes-demo/demo.bb", "SUMMARY = \"demo\"\n");
    let bitbake = directory.write(
        "fake-bitbake",
        r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  echo 'WARNING: Parse warning at /layer/recipes-demo/demo.bb:7: dynamic provider'
  exit 0
fi
if [ "$1" = "--environment" ]; then
  printf 'SUMMARY="demo"\nDESCRIPTION=""\nLICENSE="CLOSED"\nSRCREV="AUTOINC+deadbeef"\nSRC_URI="git://example.invalid/demo.git;branch=main"\n'
  exit 0
fi
exit 0
"###,
    );
    let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bitbake, permissions).unwrap();

    let build_dir = directory.path().join("build");
    let output = run([
        "lint",
        "--semantic",
        "--build-dir",
        build_dir.to_str().unwrap(),
        "--bitbake",
        bitbake.to_str().unwrap(),
        "--target",
        "demo",
        "--output",
        "json",
        recipe.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["semantic"]["parse_succeeded"], true);
    assert_eq!(report["semantic"]["build_context_source"], "explicit");
    assert_eq!(report["semantic"]["targets"][0], "demo");
    let diagnostics = report["diagnostics"].as_array().unwrap();
    let bitbake_diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["rule_id"] == "BBT019")
        .unwrap();
    assert_eq!(bitbake_diagnostic["path"], "/layer/recipes-demo/demo.bb");
    assert_eq!(bitbake_diagnostic["line"], 7);
    assert!(
        bitbake_diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("dynamic provider")
    );
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["rule_id"] == "BBT012"
            && diagnostic["message"]
                .as_str()
                .unwrap()
                .contains("DESCRIPTION")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["rule_id"] == "BBT004"
            && diagnostic["message"].as_str().unwrap().contains("AUTOREV")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["rule_id"] == "BBT015"
            && diagnostic["message"]
                .as_str()
                .unwrap()
                .contains("transport protocol")
    }));
}

#[cfg(unix)]
#[test]
fn semantic_lint_requires_the_semantic_flag_for_bitbake_options() {
    let directory = TemporaryDirectory::new("lint-bitbake-flag");
    let file = directory.write("example.bb", "SUMMARY = \"demo\"\n");

    let output = run([
        "lint",
        "--build-dir",
        directory.path().to_str().unwrap(),
        file.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("require --semantic"));
    assert!(output.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn semantic_lint_turns_silent_bitbake_failures_into_blocking_findings() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TemporaryDirectory::new("lint-bitbake-target-failure");
    directory.write("build/conf/local.conf", "MACHINE = \"qemux86-64\"\n");
    directory.write("build/conf/bblayers.conf", "BBLAYERS = \"/layer\"\n");
    let recipe = directory.write("recipes-demo/demo.bb", "SUMMARY = \"demo\"\n");
    let bitbake = directory.write(
        "fake-bitbake",
        r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  exit 0
fi
if [ "$1" = "--environment" ]; then
  exit 1
fi
exit 0
"###,
    );
    let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bitbake, permissions).unwrap();

    let output = run([
        "lint",
        "--semantic",
        "--build-dir",
        directory.path().join("build").to_str().unwrap(),
        "--bitbake",
        bitbake.to_str().unwrap(),
        "--target",
        "demo",
        "--output",
        "json",
        recipe.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| {
                diagnostic["rule_id"] == "BBT019"
                    && diagnostic["message"]
                        .as_str()
                        .unwrap()
                        .contains("target query failed")
            })
    );
}

#[cfg(unix)]
#[test]
fn semantic_command_discovers_build_context_from_project_directory() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TemporaryDirectory::new("semantic-discovery-cli");
    directory.write("build/conf/local.conf", "MACHINE = \"qemux86-64\"\n");
    directory.write("build/conf/bblayers.conf", "BBLAYERS = \"/layer\"\n");
    let bitbake = directory.write(
        "fake-bitbake",
        r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
exit 0
"###,
    );
    let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bitbake, permissions).unwrap();

    let output = run([
        "semantic",
        "--project-dir",
        directory.path().to_str().unwrap(),
        "--bitbake",
        bitbake.to_str().unwrap(),
        "--output",
        "json",
    ]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let project_dir = fs::canonicalize(directory.path()).unwrap();
    assert_eq!(report["project_dir"], project_dir.to_str().unwrap());
    assert_eq!(
        report["build_dir"],
        project_dir.join("build").to_str().unwrap()
    );
    assert_eq!(report["build_context_source"], "discovered");
}

#[cfg(unix)]
#[test]
fn semantic_command_uses_configured_build_context_and_bitbake() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TemporaryDirectory::new("semantic-configured-context");
    directory.write("build/conf/local.conf", "MACHINE = \"qemux86-64\"\n");
    directory.write("build/conf/bblayers.conf", "BBLAYERS = \"/layer\"\n");
    let bitbake = directory.write(
        "fake-bitbake",
        r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
exit 0
"###,
    );
    let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bitbake, permissions).unwrap();
    let config = directory.write(
        ".bbtidy.toml",
        "[semantic]\nbuild_dir = \"build\"\nbitbake = \"./fake-bitbake\"\n",
    );

    let output = run([
        "--config",
        config.to_str().unwrap(),
        "semantic",
        "--output",
        "json",
    ]);

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["build_context_source"], "configured");
    let project_dir = fs::canonicalize(directory.path()).unwrap();
    assert_eq!(
        report["build_dir"],
        project_dir.join("build").to_str().unwrap()
    );
    assert_eq!(report["parse_succeeded"], true);
}

#[test]
fn semantic_command_reports_missing_discovered_context() {
    let directory = TemporaryDirectory::new("semantic-missing-context");

    let output = run([
        "semantic",
        "--project-dir",
        directory.path().to_str().unwrap(),
        "--output",
        "json",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("could not discover a BitBake build directory")
    );
    assert!(output.stdout.is_empty());
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
