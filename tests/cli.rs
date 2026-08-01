use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const UNFORMATTED: &str = "SUMMARY=\"demo\"\n";
const FORMATTED: &str = "SUMMARY = \"demo\"\n";

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
fn directories_are_recursive_filtered_and_deterministic() {
    let directory = TemporaryDirectory::new("directory");
    let nested = directory.write("nested/a.conf", UNFORMATTED);
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
fn a_batch_format_error_prevents_all_writes() {
    let directory = TemporaryDirectory::new("batch-error");
    let valid = directory.write("valid.bb", UNFORMATTED);
    let malformed = directory.write("malformed.conf", "BROKEN = \"value\n");

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
    let nested = directory.write("nested/a.conf", "A = \"a\"");
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
