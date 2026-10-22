//! Small helpers shared across modules.

use std::io::Write;
use std::path::Path;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Writes `data` to `path` with mode 0600 through a temporary file and a rename,
/// so a crash never leaves a half-written file.
pub fn write_private_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Runs `f` on its own thread and gives up waiting after `d`. A call that never returns
/// (libgit2 reading a FIFO in a hostile repo, say) then costs one parked thread instead
/// of stalling the caller.
pub fn with_deadline<T: Send + 'static>(
    d: std::time::Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("deadline".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(d).ok()
}

/// Text cut to at most `max` characters, on a character boundary.
pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

/// True for control characters and bidirectional-text controls, which can make a
/// path or title display as something it isn't.
pub fn has_control_or_bidi(s: &str) -> bool {
    s.chars().any(|c| {
        c.is_control()
            || matches!(c,
                '\u{200E}' | '\u{200F}' | '\u{061C}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn atomic_write_replaces_and_sets_mode() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.json");
        std::fs::write(&p, "old").unwrap();
        write_private_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 5), "hi");
    }

    #[test]
    fn spots_control_and_bidi_characters() {
        assert!(has_control_or_bidi("a\nb"));
        assert!(has_control_or_bidi("a\u{202E}b"));
        assert!(has_control_or_bidi("a\u{2066}b"));
        assert!(!has_control_or_bidi("/Users/x/My Repo (2)"));
    }
}
