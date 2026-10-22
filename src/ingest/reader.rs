//! Reads new lines from a transcript, resuming from the saved byte offset.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::parse::{Line, Parsed, parse_line};

/// Lines longer than this are skipped without parsing.
pub const MAX_LINE: usize = 8 * 1024 * 1024;

const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub inode: u64,
    pub offset: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub position: Position,
    /// The file was replaced or shrank, so it was read again from the start.
    pub restarted: bool,
    pub lines: u64,
    pub malformed: u64,
    pub oversized: u64,
}

/// Opens a transcript safely: never follows a symlink, never blocks on a pipe, and
/// refuses anything that isn't a regular file.
pub fn open_regular(path: &Path) -> io::Result<File> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !f.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(f)
}

/// Reads complete lines after `prev` and hands each parsed one to `on_line`.
/// A trailing partial line is left for next time.
pub fn read_new(
    path: &Path,
    prev: Option<Position>,
    mut on_line: impl FnMut(Line),
) -> io::Result<Summary> {
    let mut file = open_regular(path)?;
    let meta = file.metadata()?;
    let inode = meta.ino();
    let mut summary = Summary::default();
    let mut line_start = match prev {
        Some(p) if p.inode == inode && p.offset <= meta.len() => p.offset,
        Some(_) => {
            summary.restarted = true;
            0
        }
        None => 0,
    };
    file.seek(SeekFrom::Start(line_start))?;

    // The current line so far: its length, and its bytes unless it is being skipped.
    let mut line_len = 0usize;
    let mut pending: Vec<u8> = Vec::new();
    let mut skipping = false;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut chunk = &buf[..n];
        loop {
            match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    if !skipping {
                        if line_len + i > MAX_LINE {
                            summary.oversized += 1;
                        } else {
                            pending.extend_from_slice(&chunk[..i]);
                            handle(&pending, &mut summary, &mut on_line);
                        }
                    }
                    line_start += (line_len + i + 1) as u64;
                    line_len = 0;
                    pending.clear();
                    skipping = false;
                    chunk = &chunk[i + 1..];
                }
                None => {
                    if !skipping {
                        if line_len + chunk.len() > MAX_LINE {
                            summary.oversized += 1;
                            skipping = true;
                            pending.clear();
                        } else {
                            pending.extend_from_slice(chunk);
                        }
                    }
                    line_len += chunk.len();
                    break;
                }
            }
        }
    }
    let offset = line_start;
    summary.position = Position { inode, offset };
    Ok(summary)
}

fn handle(bytes: &[u8], summary: &mut Summary, on_line: &mut impl FnMut(Line)) {
    let trimmed = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    if trimmed.iter().all(u8::is_ascii_whitespace) {
        return;
    }
    match parse_line(trimmed) {
        Parsed::Line(l) => {
            summary.lines += 1;
            on_line(*l);
        }
        Parsed::Ignored => summary.lines += 1,
        Parsed::Malformed => summary.malformed += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn prompts(path: &Path, prev: Option<Position>) -> (Vec<String>, Summary) {
        let mut out = Vec::new();
        let s = read_new(path, prev, |l| {
            if let Some(p) = l.prompt {
                out.push(p)
            }
        })
        .unwrap();
        (out, s)
    }

    fn user(text: &str) -> String {
        format!("{{\"type\":\"user\",\"message\":{{\"content\":{text:?}}}}}\n")
    }

    #[test]
    fn resumes_from_the_saved_offset() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, user("one") + &user("two")).unwrap();
        let (got, s1) = prompts(&p, None);
        assert_eq!(got, vec!["one", "two"]);

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(user("three").as_bytes()).unwrap();
        let (got, s2) = prompts(&p, Some(s1.position));
        assert_eq!(got, vec!["three"]);
        assert!(!s2.restarted);
        assert_eq!(s2.position.offset, std::fs::metadata(&p).unwrap().len());
    }

    #[test]
    fn leaves_a_partial_last_line_for_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        let full = user("one");
        let partial = user("two");
        std::fs::write(&p, format!("{full}{}", &partial[..10])).unwrap();
        let (got, s1) = prompts(&p, None);
        assert_eq!(got, vec!["one"]);
        assert_eq!(s1.position.offset, full.len() as u64);

        std::fs::write(&p, format!("{full}{partial}")).unwrap();
        let (got, _) = prompts(&p, Some(s1.position));
        assert_eq!(got, vec!["two"]);
    }

    #[test]
    fn rereads_a_file_that_shrank() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, user("one") + &user("two")).unwrap();
        let (_, s1) = prompts(&p, None);
        // Truncated in place: same inode, shorter file.
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_len(0).unwrap();
        drop(f);
        std::fs::write(&p, user("new")).unwrap();
        let (got, s2) = prompts(&p, Some(s1.position));
        assert!(s2.restarted);
        assert_eq!(got, vec!["new"]);
    }

    #[test]
    fn rereads_a_replaced_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, user("one")).unwrap();
        let (_, s1) = prompts(&p, None);
        let q = dir.path().join("new.jsonl");
        std::fs::write(&q, user("replaced") + &user("longer")).unwrap();
        std::fs::rename(&q, &p).unwrap();
        let (got, s2) = prompts(&p, Some(s1.position));
        assert!(s2.restarted);
        assert_eq!(got, vec!["replaced", "longer"]);
    }

    #[test]
    fn counts_malformed_lines_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(&p, format!("{{broken\n\n{}", user("ok"))).unwrap();
        let (got, s) = prompts(&p, None);
        assert_eq!(got, vec!["ok"]);
        assert_eq!(s.malformed, 1);
    }

    #[test]
    fn skips_oversized_lines_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        let huge = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}\n",
            "x".repeat(MAX_LINE + 100)
        );
        std::fs::write(&p, user("before") + &huge + &user("after")).unwrap();
        let (got, s) = prompts(&p, None);
        assert_eq!(got, vec!["before", "after"]);
        assert_eq!(s.oversized, 1);
        assert_eq!(s.position.offset, std::fs::metadata(&p).unwrap().len());
    }

    #[test]
    fn refuses_symlinks_and_fifos() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.jsonl");
        std::fs::write(&real, user("x")).unwrap();
        let link = dir.path().join("link.jsonl");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(read_new(&link, None, |_| {}).is_err());

        let fifo = dir.path().join("fifo.jsonl");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_new(&fifo, None, |_| {}).is_err());
    }
}
