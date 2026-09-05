use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bbtidy-transaction-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(parent_of(&path)).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn artifacts(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".bbtidy.")
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(path: &Path) -> WriteRequest<'_> {
    WriteRequest {
        path,
        original: "A=\"a\"\n",
        replacement: "A = \"a\"\n",
    }
}

fn pair(fixture: &Fixture) -> (PathBuf, PathBuf) {
    (
        fixture.write("a.bb", "A=\"a\"\n"),
        fixture.write("b.bb", "A=\"a\"\n"),
    )
}

#[derive(Default)]
struct Faults {
    replacements: usize,
    fail_replacements: Vec<usize>,
    syncs: usize,
    fail_sync: Option<usize>,
    concurrent_edit: Option<(PathBuf, String)>,
}

impl CommitFilesystem for Faults {
    fn replace(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        self.replacements += 1;
        if self.fail_replacements.contains(&self.replacements) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected replacement failure",
            ));
        }
        fs::rename(source, destination)?;
        if let Some((path, content)) = self.concurrent_edit.take() {
            fs::write(path, content)?;
        }
        Ok(())
    }

    fn sync_parent(&mut self, parent: &Path) -> io::Result<()> {
        self.syncs += 1;
        if self.fail_sync == Some(self.syncs) {
            return Err(io::Error::other("injected sync failure"));
        }
        NativeFilesystem.sync_parent(parent)
    }
}

#[test]
fn commits_multiple_files_and_removes_all_staging_files() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    Transaction::stage([request(&a), request(&b)])
        .unwrap()
        .commit()
        .unwrap();
    for path in [&a, &b] {
        assert_eq!(fs::read_to_string(path).unwrap(), "A = \"a\"\n");
    }
    assert!(fixture.artifacts().is_empty());
}

#[test]
fn dropping_uncommitted_transaction_cleans_its_files() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    assert_eq!(fixture.artifacts().len(), 4);
    drop(transaction);
    assert!(fixture.artifacts().is_empty());
    assert_eq!(fs::read_to_string(a).unwrap(), "A=\"a\"\n");
}

#[test]
fn late_staging_io_error_cleans_earlier_files() {
    let fixture = Fixture::new();
    let a = fixture.write("a.bb", "A=\"a\"\n");
    let absent = fixture.0.join("absent.bb");
    assert!(Transaction::stage([request(&a), request(&absent)]).is_err());
    assert!(fixture.artifacts().is_empty());
    assert_eq!(fs::read_to_string(a).unwrap(), "A=\"a\"\n");
}

#[test]
fn read_only_source_fails_before_commit_and_cleans_staging() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let original_permissions = fs::metadata(&b).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&b, readonly).unwrap();
    let result = Transaction::stage([request(&a), request(&b)]);
    fs::set_permissions(&b, original_permissions).unwrap();
    assert_eq!(
        result.err().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(fixture.artifacts().is_empty());
    assert_eq!(fs::read_to_string(a).unwrap(), "A=\"a\"\n");
}

#[test]
fn source_changed_before_staging_is_preserved() {
    let fixture = Fixture::new();
    let a = fixture.write("a.bb", "concurrent edit\n");
    assert!(Transaction::stage([request(&a)]).is_err());
    assert_eq!(fs::read_to_string(a).unwrap(), "concurrent edit\n");
    assert!(fixture.artifacts().is_empty());
}

#[test]
fn later_concurrent_change_rolls_back_earlier_replacement() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    fs::write(&b, "concurrent edit\n").unwrap();
    assert!(transaction.commit().is_err());
    assert_eq!(fs::read_to_string(a).unwrap(), "A=\"a\"\n");
    assert_eq!(fs::read_to_string(b).unwrap(), "concurrent edit\n");
    assert!(fixture.artifacts().is_empty());
}

#[test]
fn replacement_failure_rolls_back_without_partial_output() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    let mut faults = Faults {
        fail_replacements: vec![2],
        ..Faults::default()
    };
    assert!(transaction.commit_with(&mut faults).is_err());
    for path in [&a, &b] {
        assert_eq!(fs::read_to_string(path).unwrap(), "A=\"a\"\n");
    }
    assert_eq!(faults.replacements, 3);
    assert!(fixture.artifacts().is_empty());
}

#[test]
fn sync_failure_after_replacement_rolls_back() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    let mut faults = Faults {
        fail_sync: Some(1),
        ..Faults::default()
    };
    assert!(transaction.commit_with(&mut faults).is_err());
    for path in [&a, &b] {
        assert_eq!(fs::read_to_string(path).unwrap(), "A=\"a\"\n");
    }
    assert!(fixture.artifacts().is_empty());
}

#[test]
fn failed_restore_keeps_current_file_and_reports_recovery_backup() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    let mut faults = Faults {
        fail_replacements: vec![2, 3],
        ..Faults::default()
    };
    let error = transaction
        .commit_with(&mut faults)
        .unwrap_err()
        .to_string();
    assert_eq!(fs::read_to_string(&a).unwrap(), "A = \"a\"\n");
    assert_eq!(fs::read_to_string(&b).unwrap(), "A=\"a\"\n");
    let artifacts = fixture.artifacts();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(fs::read_to_string(&artifacts[0]).unwrap(), "A=\"a\"\n");
    assert!(error.contains(&format!("original preserved at {}", artifacts[0].display())));
}

#[test]
fn rollback_preserves_edits_made_after_our_replacement() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    let mut faults = Faults {
        fail_replacements: vec![2],
        concurrent_edit: Some((a.clone(), "new user edit\n".to_owned())),
        ..Faults::default()
    };
    let error = transaction
        .commit_with(&mut faults)
        .unwrap_err()
        .to_string();
    assert_eq!(fs::read_to_string(&a).unwrap(), "new user edit\n");
    assert!(error.contains("original preserved at"));
    assert_eq!(fixture.artifacts().len(), 1);
}

#[test]
fn cleanup_failure_reports_that_changes_were_committed() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    let mut faults = Faults {
        fail_sync: Some(3),
        ..Faults::default()
    };
    let error = transaction
        .commit_with(&mut faults)
        .unwrap_err()
        .to_string();
    assert!(error.contains("all source changes were committed"));
    for path in [&a, &b] {
        assert_eq!(fs::read_to_string(path).unwrap(), "A = \"a\"\n");
    }
    assert!(fixture.artifacts().is_empty());
}

#[cfg(unix)]
#[test]
fn preserves_unix_permissions_on_commit_and_rollback() {
    use std::os::unix::fs::PermissionsExt;
    for fail in [false, true] {
        let fixture = Fixture::new();
        let (a, b) = pair(&fixture);
        fs::set_permissions(&a, fs::Permissions::from_mode(0o750)).unwrap();
        let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
        let mut faults = Faults {
            fail_replacements: if fail { vec![2] } else { vec![] },
            ..Faults::default()
        };
        assert_eq!(transaction.commit_with(&mut faults).is_err(), fail);
        assert_eq!(fs::metadata(a).unwrap().permissions().mode() & 0o777, 0o750);
        assert!(fixture.artifacts().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn unwritable_directory_leaves_all_sources_unchanged() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    } // root bypasses directory mode checks
    let fixture = Fixture::new();
    let a = fixture.write("a.bb", "A=\"a\"\n");
    let b = fixture.write("locked/b.bb", "A=\"a\"\n");
    let directory = parent_of(&b);
    let permissions = fs::metadata(directory).unwrap().permissions();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500)).unwrap();
    let result = Transaction::stage([request(&a), request(&b)]);
    fs::set_permissions(directory, permissions).unwrap();
    assert_eq!(
        result.err().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(fs::read_to_string(a).unwrap(), "A=\"a\"\n");
    assert!(fixture.artifacts().is_empty());
}

#[cfg(unix)]
#[test]
fn symlink_introduced_before_commit_is_not_replaced_or_followed() {
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let target = fixture.write("target.bb", "untouched\n");
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    fs::remove_file(&b).unwrap();
    std::os::unix::fs::symlink(&target, &b).unwrap();
    assert!(transaction.commit().is_err());
    assert!(fs::symlink_metadata(&b).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(target).unwrap(), "untouched\n");
    assert_eq!(fs::read_to_string(a).unwrap(), "A=\"a\"\n");
    assert!(fixture.artifacts().is_empty());
}

#[cfg(windows)]
#[test]
fn windows_sharing_violation_rolls_back_earlier_file() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let (a, b) = pair(&fixture);
    let transaction = Transaction::stage([request(&a), request(&b)]).unwrap();
    // Permit reads for preflight, but deny deletion/rename of the second file.
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&b)
        .unwrap();
    assert!(transaction.commit().is_err());
    drop(locked);
    for path in [&a, &b] {
        assert_eq!(fs::read_to_string(path).unwrap(), "A=\"a\"\n");
    }
    assert!(fixture.artifacts().is_empty());
}
