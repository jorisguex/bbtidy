use bbtidy::format;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct MalformedCase {
    file_name: &'static str,
    line: usize,
    message: &'static str,
}

const CASES: [MalformedCase; 3] = [
    MalformedCase {
        file_name: "unclosed-function.bb",
        line: 2,
        message: "function body has no closing brace",
    },
    MalformedCase {
        file_name: "unclosed-quote.conf",
        line: 1,
        message: "top-level assignment contains an unclosed quote",
    },
    MalformedCase {
        file_name: "unterminated-continuation.inc",
        line: 1,
        message: "statement ends with an unterminated continuation",
    },
];

#[test]
fn malformed_corpus_returns_structured_errors() {
    for case in &CASES {
        let input = read_malformed(case.file_name);
        let error =
            format(&input).expect_err(&format!("{} unexpectedly formatted", case.file_name));

        assert_eq!(error.line(), case.line, "wrong line for {}", case.file_name);
        assert_eq!(
            error.message(),
            case.message,
            "wrong error for {}",
            case.file_name
        );
    }
}

#[test]
fn cli_never_partially_writes_malformed_files() {
    let temporary_directory = create_temporary_directory();

    for case in &CASES {
        let original = read_malformed(case.file_name);
        let temporary_file = temporary_directory.join(case.file_name);
        fs::write(&temporary_file, &original).unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_bbtidy"))
            .args(["format", "--write"])
            .arg(&temporary_file)
            .output()
            .unwrap();

        assert!(
            !output.status.success(),
            "{} unexpectedly succeeded",
            case.file_name
        );
        assert_eq!(
            fs::read_to_string(&temporary_file).unwrap(),
            original,
            "{} was partially rewritten",
            case.file_name
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(case.message),
            "missing diagnostic for {}: {}",
            case.file_name,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let remaining_files = fs::read_dir(&temporary_directory).unwrap().count();
    assert_eq!(
        remaining_files,
        CASES.len(),
        "temporary output was left behind"
    );
    fs::remove_dir_all(temporary_directory).unwrap();
}

fn read_malformed(file_name: &str) -> String {
    fs::read_to_string(malformed_root().join(file_name))
        .unwrap_or_else(|error| panic!("failed to read {file_name}: {error}"))
}

fn malformed_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/malformed")
}

fn create_temporary_directory() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("bbtidy-malformed-{}-{unique}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    directory
}
