use bbtidy::format;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURES: [&str; 5] = [
    "conf/layer.conf",
    "classes/example.bbclass",
    "recipes-example/example/example.inc",
    "recipes-example/example/example_1.0.bb",
    "recipes-example/example/example_%.bbappend",
];

#[test]
fn corpus_matches_golden_output() {
    for relative_path in FIXTURES {
        let input = read_fixture("input", relative_path);
        let expected = read_fixture("expected", relative_path);
        let actual = format(&input).unwrap_or_else(|error| {
            panic!("failed to format {relative_path}: {error}");
        });

        assert_eq!(actual, expected, "golden mismatch for {relative_path}");
    }
}

#[test]
fn formatted_corpus_is_idempotent() {
    for relative_path in FIXTURES {
        let expected = read_fixture("expected", relative_path);
        let actual = format(&expected).unwrap_or_else(|error| {
            panic!("failed to reformat {relative_path}: {error}");
        });

        assert_eq!(
            actual, expected,
            "non-idempotent output for {relative_path}"
        );
    }
}

#[test]
fn corpus_preserves_every_opaque_region_byte_for_byte() {
    for relative_path in FIXTURES {
        let input = read_fixture("input", relative_path);
        let formatted = format(&input).unwrap();
        let before = opaque_regions(&input, relative_path);
        let after = opaque_regions(&formatted, relative_path);

        assert!(!before.is_empty(), "no opaque regions in {relative_path}");
        assert_eq!(
            after, before,
            "formatter changed an opaque region in {relative_path}"
        );
    }
}

#[test]
fn corpus_covers_each_supported_bitbake_file_type() {
    let extensions: BTreeSet<&str> = FIXTURES
        .iter()
        .map(|path| Path::new(path).extension().unwrap().to_str().unwrap())
        .collect();

    assert_eq!(
        extensions,
        BTreeSet::from(["bb", "bbappend", "bbclass", "conf", "inc"])
    );
}

#[test]
fn cli_formats_the_corpus_tree_to_golden_output() {
    let temporary_directory = create_temporary_directory("corpus");

    for relative_path in FIXTURES {
        let temporary_file = temporary_directory.join(relative_path);
        fs::create_dir_all(temporary_file.parent().unwrap()).unwrap();
        fs::write(&temporary_file, read_fixture("input", relative_path)).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_bbtidy"))
        .args(["format", "--write"])
        .arg(&temporary_directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for relative_path in FIXTURES {
        assert_eq!(
            fs::read_to_string(temporary_directory.join(relative_path)).unwrap(),
            read_fixture("expected", relative_path),
            "CLI golden mismatch for {relative_path}"
        );
    }

    fs::remove_dir_all(temporary_directory).unwrap();
}

fn read_fixture(tree: &str, relative_path: &str) -> String {
    fs::read_to_string(fixture_root().join(tree).join(relative_path))
        .unwrap_or_else(|error| panic!("failed to read {tree}/{relative_path}: {error}"))
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus")
}

fn opaque_regions(text: &str, fixture: &str) -> BTreeMap<String, String> {
    const START: &str = "# bbtidy-corpus:opaque-start ";
    const END: &str = "# bbtidy-corpus:opaque-end ";

    let mut regions = BTreeMap::new();
    let mut current: Option<(String, String)> = None;

    for line in text.split_inclusive('\n') {
        if let Some(name) = line.trim_end().strip_prefix(START) {
            assert!(
                current.is_none(),
                "nested opaque region {name} in {fixture}"
            );
            current = Some((name.to_owned(), String::new()));
        }

        if let Some((_, contents)) = current.as_mut() {
            contents.push_str(line);
        }

        if let Some(name) = line.trim_end().strip_prefix(END) {
            let (start_name, contents) = current
                .take()
                .unwrap_or_else(|| panic!("opaque end without start in {fixture}"));
            assert_eq!(name, start_name, "opaque marker mismatch in {fixture}");
            assert!(
                regions.insert(start_name, contents).is_none(),
                "duplicate opaque marker {name} in {fixture}"
            );
        }
    }

    assert!(current.is_none(), "unclosed opaque region in {fixture}");
    regions
}

fn create_temporary_directory(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("bbtidy-{label}-{}-{unique}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    directory
}
