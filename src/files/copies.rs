//! Copies of docs and outputs found in temporary folders, which macOS deletes
//! (design §10 and §20.5).

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub per_file_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Copied { path: PathBuf, size: u64 },
    TooLarge,
    NotAllowed(&'static str),
}

/// Copies `src` to `<copies>/<block_id>/<file_id>-<name>` with mode 0600.
///
/// The source must be a regular file owned by the current user. It is opened with
/// `O_NOFOLLOW | O_NONBLOCK`, so a symlink or pipe planted in a shared temporary folder
/// is refused rather than followed or waited on.
pub fn copy_file(
    src: &Path,
    copies: &Path,
    block_id: i64,
    file_id: i64,
    limits: Limits,
) -> io::Result<Outcome> {
    let mut f = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(src)
    {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Ok(Outcome::NotAllowed("symlink"));
        }
        Err(e) => return Err(e),
    };
    let meta = f.metadata()?;
    if !meta.file_type().is_file() {
        return Ok(Outcome::NotAllowed("not a regular file"));
    }
    if meta.uid() != unsafe { libc::getuid() } {
        return Ok(Outcome::NotAllowed("owned by another user"));
    }
    if meta.len() > limits.per_file_bytes {
        return Ok(Outcome::TooLarge);
    }

    let dir = copies.join(block_id.to_string());
    crate::paths::ensure_private_dir(copies)?;
    crate::paths::ensure_private_dir(&dir)?;
    let name = src
        .file_name()
        .map(|n| safe_name(&n.to_string_lossy()))
        .unwrap_or_else(|| "file".into());
    let dest = dir.join(format!("{file_id}-{name}"));
    let tmp = dir.join(format!(".{file_id}.tmp-{}", std::process::id()));

    let result = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        // Never copy more than the limit, even if the file grows while being read.
        let mut limited = (&mut f).take(limits.per_file_bytes + 1);
        let n = io::copy(&mut limited, &mut out)?;
        if n > limits.per_file_bytes {
            return Ok(None);
        }
        out.flush()?;
        std::fs::rename(&tmp, &dest)?;
        Ok(Some(n))
    })();
    match result {
        Ok(Some(size)) => Ok(Outcome::Copied { path: dest, size }),
        Ok(None) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(Outcome::TooLarge)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Keeps file names to a plain, displayable set of characters.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "file".into()
    } else {
        crate::util::truncate_chars(trimmed, 120)
    }
}

/// Removes copies until the total is within `limit`: copies from archived blocks
/// first, then the oldest. Returns the removed paths.
pub fn enforce_total(
    copies: &Path,
    limit: u64,
    archived_blocks: &HashSet<i64>,
) -> io::Result<Vec<PathBuf>> {
    struct Entry {
        path: PathBuf,
        size: u64,
        mtime: i64,
        archived: bool,
    }
    let mut entries = Vec::new();
    let Ok(blocks) = std::fs::read_dir(copies) else {
        return Ok(Vec::new());
    };
    for block in blocks.flatten() {
        let block_id: Option<i64> = block.file_name().to_str().and_then(|s| s.parse().ok());
        let Some(block_id) = block_id else { continue };
        for f in std::fs::read_dir(block.path())?.flatten() {
            let meta = f.metadata()?;
            if meta.is_file() {
                entries.push(Entry {
                    path: f.path(),
                    size: meta.len(),
                    mtime: meta.mtime(),
                    archived: archived_blocks.contains(&block_id),
                });
            }
        }
    }
    let mut total: u64 = entries.iter().map(|e| e.size).sum();
    // Archived first, then oldest first.
    entries.sort_by_key(|e| (!e.archived, e.mtime));
    let mut removed = Vec::new();
    for e in entries {
        if total <= limit {
            break;
        }
        std::fs::remove_file(&e.path)?;
        total -= e.size;
        removed.push(e.path);
    }
    Ok(removed)
}

/// Total size of all copies.
pub fn total_size(copies: &Path) -> u64 {
    let Ok(blocks) = std::fs::read_dir(copies) else {
        return 0;
    };
    blocks
        .flatten()
        .filter_map(|b| std::fs::read_dir(b.path()).ok())
        .flat_map(|d| d.flatten())
        .filter_map(|f| f.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Reads the whole of a small file safely (used to compare a copy with its source).
pub fn read_small(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn limits(per_file: u64, total: u64) -> Limits {
        Limits {
            per_file_bytes: per_file,
            total_bytes: total,
        }
    }

    #[test]
    fn copies_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("report.html");
        std::fs::write(&src, "<html>").unwrap();
        let copies = dir.path().join("copies");
        let out = copy_file(&src, &copies, 7, 3, limits(100, 1000)).unwrap();
        let Outcome::Copied { path, size } = out else {
            panic!("{out:?}")
        };
        assert_eq!(path, copies.join("7/3-report.html"));
        assert_eq!(size, 6);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "<html>");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(copies.join("7"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn refuses_symlinks_fifos_and_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let copies = dir.path().join("copies");
        let real = dir.path().join("real.html");
        std::fs::write(&real, "0123456789").unwrap();
        let link = dir.path().join("link.html");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            copy_file(&link, &copies, 1, 1, limits(100, 1000)).unwrap(),
            Outcome::NotAllowed("symlink")
        );
        let fifo = dir.path().join("fifo.html");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(
            copy_file(&fifo, &copies, 1, 2, limits(100, 1000)).unwrap(),
            Outcome::NotAllowed("not a regular file")
        );
        assert_eq!(
            copy_file(&real, &copies, 1, 3, limits(5, 1000)).unwrap(),
            Outcome::TooLarge
        );
    }

    #[test]
    fn odd_names_are_made_safe() {
        assert_eq!(safe_name("a\nb$(x).html"), "a_b__x_.html");
        assert_eq!(safe_name("..hidden"), "hidden");
        assert_eq!(safe_name("..."), "file");
    }

    #[test]
    fn evicts_archived_blocks_first_then_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let copies = dir.path();
        let write = |block: i64, name: &str, size: usize, age: i64| {
            let d = copies.join(block.to_string());
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join(name);
            std::fs::write(&p, vec![b'x'; size]).unwrap();
            let t = libc::timespec {
                tv_sec: 1_700_000_000 - age,
                tv_nsec: 0,
            };
            let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
            unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [t, t].as_ptr(), 0) };
            p
        };
        let old_live = write(1, "1-old.csv", 10, 100);
        let new_live = write(1, "2-new.csv", 10, 1);
        let archived = write(2, "3-arch.csv", 10, 0);
        let removed = enforce_total(copies, 15, &HashSet::from([2])).unwrap();
        assert_eq!(removed, vec![archived.clone(), old_live.clone()]);
        assert!(new_live.exists());
        assert_eq!(total_size(copies), 10);
    }
}
