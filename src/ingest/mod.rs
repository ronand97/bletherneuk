//! Reading Claude Code transcripts into the index (design §8).

pub mod parse;
pub mod reader;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::config::Config;
use crate::files::{self, Kind, PathFinder};
use crate::paths::Paths;
use parse::Line;
use reader::Position;

/// The fallback title is the first prompt cut to this many characters.
const PROMPT_TITLE_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transcript {
    /// `projects/<folder>/<session>.jsonl`
    Main { session_id: String },
    /// Anything under `projects/<folder>/<session>/`, such as subagent transcripts.
    Subagent { session_id: String },
}

impl Transcript {
    pub fn session_id(&self) -> &str {
        match self {
            Transcript::Main { session_id } | Transcript::Subagent { session_id } => session_id,
        }
    }
}

pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Works out which chat a `.jsonl` file under `projects` belongs to.
pub fn classify_transcript(projects: &Path, path: &Path) -> Option<Transcript> {
    if path.extension()? != "jsonl" {
        return None;
    }
    let rel = path.strip_prefix(projects).ok()?;
    let parts: Vec<&str> = rel.iter().map(|p| p.to_str()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [_folder, file] => {
            let id = file.strip_suffix(".jsonl")?;
            is_uuid(id).then(|| Transcript::Main {
                session_id: id.to_string(),
            })
        }
        [_folder, session, _, ..] if is_uuid(session) => Some(Transcript::Subagent {
            session_id: session.to_string(),
        }),
        _ => None,
    }
}

/// Everything learned from one read of one transcript.
#[derive(Debug, Default)]
struct Batch {
    custom_title: Option<String>,
    ai_title: Option<String>,
    first_prompt: Option<String>,
    cwds: Vec<String>,
    branch: Option<String>,
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    /// (absolute path, time)
    writes: Vec<(PathBuf, Option<i64>)>,
    /// (absolute path, found_by)
    mentions: Vec<(PathBuf, &'static str)>,
}

impl Batch {
    fn add(&mut self, line: Line, finder: &PathFinder, home: &Path, subagent: bool) {
        let cwd = line.cwd.clone().or_else(|| self.cwds.last().cloned());
        let cwd_path = cwd.as_deref().map(Path::new);
        for (_tool, raw) in &line.writes {
            if let Some(p) = files::resolve(raw, cwd_path, home) {
                self.writes.push((p, line.timestamp));
            }
        }
        for cmd in &line.commands {
            for p in finder.find(cmd, cwd_path, home) {
                self.mentions.push((p, "command"));
            }
        }
        for out in &line.outputs {
            for p in finder.find(out, cwd_path, home) {
                self.mentions.push((p, "output"));
            }
        }
        if subagent {
            return;
        }
        if let Some(t) = line.custom_title {
            self.custom_title = Some(t);
        }
        if let Some(t) = line.ai_title {
            self.ai_title = Some(t);
        }
        if self.first_prompt.is_none() {
            self.first_prompt = line.prompt;
        }
        if let Some(c) = line.cwd
            && self.cwds.last() != Some(&c)
        {
            self.cwds.push(c);
        }
        if let Some(b) = line.git_branch {
            self.branch = Some(b);
        }
        if let Some(ts) = line.timestamp {
            self.first_ts = Some(self.first_ts.map_or(ts, |f| f.min(ts)));
            self.last_ts = Some(self.last_ts.map_or(ts, |l| l.max(ts)));
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub lines: u64,
    pub malformed: u64,
    pub oversized: u64,
}

pub struct Ingestor<'a> {
    pub config: &'a Config,
    pub paths: &'a Paths,
    finder: PathFinder,
}

impl<'a> Ingestor<'a> {
    pub fn new(config: &'a Config, paths: &'a Paths) -> Ingestor<'a> {
        Ingestor {
            config,
            paths,
            finder: PathFinder::new(&config.outputs.extensions),
        }
    }

    /// Reads whatever is new in one transcript. Returns the chat it belongs to, or
    /// `None` for files that aren't transcripts.
    pub fn ingest_file(
        &self,
        conn: &mut Connection,
        path: &Path,
        now_ms: i64,
    ) -> anyhow::Result<Option<(String, Counts)>> {
        let Some(t) = classify_transcript(&self.paths.claude_projects(), path) else {
            return Ok(None);
        };
        let key = path.to_string_lossy().into_owned();
        let prev: Option<Position> = conn
            .query_row(
                "SELECT inode, byte_offset FROM ingest_offsets WHERE path = ?1",
                [&key],
                |r| {
                    Ok(Position {
                        inode: r.get::<_, i64>(0)? as u64,
                        offset: r.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .optional()?;

        let subagent = matches!(t, Transcript::Subagent { .. });
        let mut batch = Batch::default();
        let home = self.paths.home.clone();
        let summary = reader::read_new(path, prev, |line| {
            batch.add(line, &self.finder, &home, subagent)
        })?;

        let chat_id = t.session_id().to_string();
        let tx = conn.transaction()?;
        if summary.restarted && !subagent {
            reset_chat(&tx, &chat_id)?;
        }
        ensure_chat(&tx, &chat_id)?;
        if !subagent {
            tx.execute(
                "UPDATE chats SET transcript_path = ?2, transcript_gone = 0 WHERE id = ?1",
                params![chat_id, key],
            )?;
        }
        self.apply(&tx, &chat_id, &batch, now_ms)?;
        tx.execute(
            "INSERT INTO ingest_offsets(path, inode, byte_offset, session_id) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET inode = excluded.inode, byte_offset = excluded.byte_offset",
            params![
                key,
                summary.position.inode as i64,
                summary.position.offset as i64,
                chat_id
            ],
        )?;
        bump_counter(&tx, "ingest.malformed", summary.malformed)?;
        bump_counter(&tx, "ingest.oversized", summary.oversized)?;
        tx.commit()?;
        Ok(Some((
            chat_id,
            Counts {
                lines: summary.lines,
                malformed: summary.malformed,
                oversized: summary.oversized,
            },
        )))
    }

    fn apply(
        &self,
        tx: &Connection,
        chat_id: &str,
        b: &Batch,
        now_ms: i64,
    ) -> rusqlite::Result<()> {
        if let Some(t) = &b.custom_title {
            tx.execute(
                "UPDATE chats SET title = ?2, title_source = 'custom' WHERE id = ?1",
                params![chat_id, t],
            )?;
        }
        if let Some(t) = &b.ai_title {
            tx.execute(
                "UPDATE chats SET title = ?2, title_source = 'ai'
                 WHERE id = ?1 AND COALESCE(title_source, '') != 'custom'",
                params![chat_id, t],
            )?;
        }
        if let Some(p) = &b.first_prompt {
            tx.execute(
                "UPDATE chats SET first_prompt = ?2 WHERE id = ?1 AND first_prompt IS NULL",
                params![chat_id, p],
            )?;
            tx.execute(
                "UPDATE chats SET title = ?2, title_source = 'prompt' WHERE id = ?1 AND title IS NULL",
                params![
                    chat_id,
                    crate::util::truncate_chars(&p.replace(['\n', '\r', '\t'], " "), PROMPT_TITLE_CHARS)
                ],
            )?;
        }
        if let Some(first) = b.cwds.first() {
            tx.execute(
                "UPDATE chats SET start_cwd = COALESCE(start_cwd, ?2), last_cwd = ?3 WHERE id = ?1",
                params![chat_id, first, b.cwds.last()],
            )?;
            for c in &b.cwds {
                tx.execute(
                    "INSERT OR IGNORE INTO chat_folders(chat_id, path) VALUES (?1, ?2)",
                    params![chat_id, c],
                )?;
            }
        }
        if let Some(br) = &b.branch {
            tx.execute(
                "UPDATE chats SET recorded_branch = ?2 WHERE id = ?1",
                params![chat_id, br],
            )?;
        }
        if let (Some(f), Some(l)) = (b.first_ts, b.last_ts) {
            tx.execute(
                "UPDATE chats SET first_activity = MIN(COALESCE(first_activity, ?2), ?2),
                                  last_activity = MAX(COALESCE(last_activity, ?3), ?3)
                 WHERE id = ?1",
                params![chat_id, f, l],
            )?;
        }

        for (p, ts) in &b.writes {
            let path = p.to_string_lossy();
            tx.execute(
                "INSERT INTO chat_writes(chat_id, path, count, last_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(chat_id, path) DO UPDATE
                 SET count = count + 1, last_at = COALESCE(excluded.last_at, last_at)",
                params![chat_id, path, ts],
            )?;
            if let Some(kind) = files::classify_written(p, self.config, self.paths) {
                upsert_file(tx, chat_id, p, kind, "tool", now_ms)?;
            }
        }
        for (p, found_by) in &b.mentions {
            if let Some(kind) = files::classify_mentioned(p, self.config, self.paths) {
                upsert_file(tx, chat_id, p, kind, found_by, now_ms)?;
            }
        }

        // Chats with no branch that ran in a temporary folder are mostly automated runs.
        let (branch, cwd): (Option<String>, Option<String>) = tx.query_row(
            "SELECT recorded_branch, last_cwd FROM chats WHERE id = ?1",
            [chat_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let no_branch = branch.as_deref().is_none_or(|b| b == "HEAD");
        let in_temp = cwd
            .as_deref()
            .is_some_and(|c| files::in_temp_dir(Path::new(c), self.config));
        tx.execute(
            "UPDATE chats SET hidden = ?2 WHERE id = ?1",
            params![chat_id, no_branch && in_temp],
        )?;
        Ok(())
    }

    /// Claude Code deleted a transcript. Prompt text goes; title and metadata stay.
    pub fn forget_transcript(&self, conn: &Connection, path: &Path) -> rusqlite::Result<()> {
        let key = path.to_string_lossy();
        conn.execute("DELETE FROM ingest_offsets WHERE path = ?1", [&key])?;
        if let Some(Transcript::Main { session_id }) =
            classify_transcript(&self.paths.claude_projects(), path)
        {
            conn.execute(
                "UPDATE chats SET first_prompt = NULL, transcript_gone = 1 WHERE id = ?1",
                [&session_id],
            )?;
            conn.execute(
                "UPDATE turns SET prompt = NULL WHERE chat_id = ?1",
                [&session_id],
            )?;
            conn.execute(
                "UPDATE chats SET title = NULL, title_source = NULL
                 WHERE id = ?1 AND title_source = 'prompt'",
                [&session_id],
            )?;
        }
        Ok(())
    }
}

fn ensure_chat(tx: &Connection, chat_id: &str) -> rusqlite::Result<()> {
    tx.execute("INSERT OR IGNORE INTO chats(id) VALUES (?1)", [chat_id])?;
    Ok(())
}

/// The transcript was replaced or shrank: forget what was derived from it so a fresh
/// read doesn't double-count.
fn reset_chat(tx: &Connection, chat_id: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM chat_writes WHERE chat_id = ?1", [chat_id])?;
    tx.execute("DELETE FROM chat_folders WHERE chat_id = ?1", [chat_id])?;
    tx.execute(
        "UPDATE chats SET title = NULL, title_source = NULL, first_prompt = NULL,
                          start_cwd = NULL, first_activity = NULL
         WHERE id = ?1",
        [chat_id],
    )?;
    Ok(())
}

fn upsert_file(
    tx: &Connection,
    chat_id: &str,
    path: &Path,
    kind: Kind,
    found_by: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let meta = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file());
    let size = meta.as_ref().map(|m| m.len() as i64);
    let mtime = meta.as_ref().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.mtime() * 1000
    });
    tx.execute(
        "INSERT INTO files(chat_id, path, kind, found_by, size, mtime, missing, first_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(chat_id, path) WHERE chat_id IS NOT NULL DO UPDATE
         SET size = COALESCE(excluded.size, size), mtime = COALESCE(excluded.mtime, mtime),
             missing = excluded.missing",
        params![
            chat_id,
            path.to_string_lossy(),
            kind.as_str(),
            found_by,
            size,
            mtime,
            meta.is_none(),
            now_ms
        ],
    )?;
    Ok(())
}

fn bump_counter(tx: &Connection, key: &str, by: u64) -> rusqlite::Result<()> {
    if by == 0 {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + ?2",
        params![key, by as i64],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
