//! Staged file replacement shared by formatting and safe lint fixes.
//!
//! Every temporary file is owned until it is renamed or deliberately retained
//! for recovery. Handled commit failures roll back earlier replacements; this
//! is not a crash-recovery journal or an atomic transaction across files.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct WriteRequest<'a> {
    pub path: &'a Path,
    pub original: &'a str,
    pub replacement: &'a str,
}

pub(crate) struct Transaction {
    pending: Vec<PendingWrite>,
}

struct PendingWrite {
    path: PathBuf,
    original: String,
    replacement: String,
    output: TemporaryFile,
    backup: TemporaryFile,
}

struct TemporaryFile {
    path: PathBuf,
    owned: bool,
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Transaction {
    pub(crate) fn stage<'a>(
        requests: impl IntoIterator<Item = WriteRequest<'a>>,
    ) -> io::Result<Self> {
        let mut transaction = Self {
            pending: Vec::new(),
        };
        for request in requests {
            if request.original == request.replacement {
                continue;
            }
            let metadata = validate_source(request.path, request.original)?;
            let output = temporary_file(request.path, "output", request.replacement, &metadata)?;
            let backup = temporary_file(request.path, "backup", request.original, &metadata)?;
            transaction.pending.push(PendingWrite {
                path: request.path.to_path_buf(),
                original: request.original.to_owned(),
                replacement: request.replacement.to_owned(),
                output,
                backup,
            });
        }
        Ok(transaction)
    }

    pub(crate) fn commit(self) -> io::Result<()> {
        self.commit_with(&mut NativeFilesystem)
    }

    fn commit_with(mut self, filesystem: &mut impl CommitFilesystem) -> io::Result<()> {
        let mut committed = 0;
        for item in &mut self.pending {
            let result = (|| {
                validate_source(&item.path, &item.original)?;
                filesystem.replace(&item.output.path, &item.path)?;
                item.output.owned = false;
                // Until restoration or successful cleanup, preserve the backup
                // even if unwinding interrupts the commit.
                item.backup.owned = false;
                committed += 1;
                filesystem.sync_parent(parent_of(&item.path))
            })();
            if let Err(error) = result {
                let rollback = self.rollback(committed, filesystem);
                return Err(match rollback {
                    Ok(()) => error,
                    Err(recovery) => {
                        io::Error::other(format!("{error}; rollback failed: {recovery}"))
                    }
                });
            }
        }

        let mut cleanup_errors = Vec::new();
        for item in &self.pending {
            let cleanup = fs::remove_file(&item.backup.path)
                .and_then(|()| filesystem.sync_parent(parent_of(&item.path)));
            if let Err(error) = cleanup {
                cleanup_errors.push(format!("{}: {error}", item.backup.path.display()));
            }
        }
        if cleanup_errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "all source changes were committed, but backup cleanup failed: {}",
                cleanup_errors.join("; ")
            )))
        }
    }

    fn rollback(
        &mut self,
        committed: usize,
        filesystem: &mut impl CommitFilesystem,
    ) -> io::Result<()> {
        let mut errors = Vec::new();
        for item in self.pending[..committed].iter_mut().rev() {
            // Do not overwrite edits made after our replacement. Preserve the
            // original in its backup and report its exact recovery path.
            let restored = validate_source(&item.path, &item.replacement)
                .and_then(|_| filesystem.replace(&item.backup.path, &item.path));
            match restored {
                Ok(()) => {
                    if let Err(error) = filesystem.sync_parent(parent_of(&item.path)) {
                        errors.push(format!(
                            "{} restored but directory sync failed: {error}",
                            item.path.display()
                        ));
                    }
                }
                Err(error) => errors.push(format!(
                    "could not restore {}: {error}; original preserved at {}",
                    item.path.display(),
                    item.backup.path.display()
                )),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
}

fn validate_source(path: &Path, expected: &str) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is not a regular file; refusing to replace directories or symbolic links",
                path.display()
            ),
        ));
    }
    if metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is read-only; refusing to replace it", path.display()),
        ));
    }
    if fs::read_to_string(path)? != expected {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!(
                "{} changed while writes were being prepared or committed",
                path.display()
            ),
        ));
    }
    Ok(metadata)
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn temporary_file(
    path: &Path,
    purpose: &str,
    contents: &str,
    metadata: &fs::Metadata,
) -> io::Result<TemporaryFile> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    for _ in 0..100 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent_of(path).join(format!(
            ".{}.bbtidy.{}.{purpose}.{id}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let temporary = TemporaryFile { path, owned: true };
        let result = (|| {
            file.write_all(contents.as_bytes())?;
            file.flush()?;
            file.set_permissions(metadata.permissions())?;
            file.sync_all()
        })();
        drop(file);
        result?;
        return Ok(temporary);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary output file",
    ))
}

// This small boundary permits deterministic failure injection without changing
// production filesystem behavior or exposing test controls through the CLI.
trait CommitFilesystem {
    fn replace(&mut self, source: &Path, destination: &Path) -> io::Result<()>;
    fn sync_parent(&mut self, parent: &Path) -> io::Result<()>;
}

struct NativeFilesystem;

impl CommitFilesystem for NativeFilesystem {
    fn replace(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        // Both files are in the same directory. Never unlink the destination:
        // a failed replacement must leave the current source intact.
        fs::rename(source, destination)
    }

    fn sync_parent(&mut self, parent: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            fs::File::open(parent)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            // Windows does not support the Unix directory-open/fsync sequence.
            // Output and recovery file contents were synced before replacement.
            // No cross-file power-loss durability is promised on any platform.
            let _ = parent;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
