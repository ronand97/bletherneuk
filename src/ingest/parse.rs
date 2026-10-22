//! Turns one transcript line into the few facts bletherneuk uses.
//!
//! The transcript format is not a public API, so every field is optional and anything
//! unrecognised is ignored (design §8).

use serde_json::Value;

/// Only the first 256 KB of each tool result is scanned for paths.
pub const MAX_OUTPUT_SCAN: usize = 256 * 1024;

/// Tools whose `file_path` counts as a file the chat wrote.
pub const WRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Line {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub timestamp: Option<i64>,
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    /// A prompt you typed (not a tool result, command echo or injected context).
    pub prompt: Option<String>,
    /// You pressed Esc: Claude Code logs "[Request interrupted by user…]" and sends no Stop.
    pub interrupted: bool,
    /// `(tool, file_path)` for each Write/Edit/MultiEdit/NotebookEdit call.
    pub writes: Vec<(String, String)>,
    pub commands: Vec<String>,
    /// Tool result text, each cut to [`MAX_OUTPUT_SCAN`] bytes.
    pub outputs: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    Line(Box<Line>),
    /// Valid JSON of a type bletherneuk doesn't read.
    Ignored,
    Malformed,
}

pub fn parse_line(bytes: &[u8]) -> Parsed {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return Parsed::Malformed;
    };
    let Some(obj) = v.as_object() else {
        return Parsed::Malformed;
    };
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
    let mut line = Line {
        session_id: str_field(&v, "sessionId"),
        cwd: str_field(&v, "cwd"),
        git_branch: str_field(&v, "gitBranch"),
        timestamp: str_field(&v, "timestamp").and_then(|t| parse_time(&t)),
        ..Line::default()
    };
    match kind {
        "custom-title" => line.custom_title = str_field(&v, "customTitle"),
        "ai-title" => line.ai_title = str_field(&v, "aiTitle"),
        "user" => read_user(&v, &mut line),
        "assistant" => read_assistant(&v, &mut line),
        "attachment" | "system" => {}
        _ => return Parsed::Ignored,
    }
    Parsed::Line(Box::new(line))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn is_true(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn read_user(v: &Value, line: &mut Line) {
    let content = v.pointer("/message/content");
    let mut texts = Vec::new();
    let mut has_tool_result = false;
    match content {
        Some(Value::String(s)) => texts.push(s.as_str()),
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            texts.push(t);
                        }
                    }
                    Some("tool_result") => {
                        has_tool_result = true;
                        if let Some(text) = tool_result_text(item.get("content")) {
                            line.outputs.push(text);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let injected =
        is_true(v, "isMeta") || is_true(v, "isCompactSummary") || is_true(v, "isSidechain");
    if has_tool_result || injected {
        return;
    }
    let text = texts.join("\n");
    let trimmed = text.trim();
    if trimmed.starts_with("[Request interrupted by user") {
        line.interrupted = true;
        return;
    }
    if !trimmed.is_empty() && !is_command_echo(trimmed) {
        line.prompt = Some(trimmed.to_string());
    }
}

/// Slash commands, `!` shell commands and their output are logged as user lines
/// wrapped in tags. They aren't prompts.
fn is_command_echo(text: &str) -> bool {
    const TAGS: &[&str] = &[
        "<command-name>",
        "<command-message>",
        "<command-args>",
        "<local-command-",
        "<bash-input>",
        "<bash-stdout>",
        "<bash-stderr>",
        "<system-reminder>",
        "<task-notification>",
    ];
    TAGS.iter().any(|t| text.starts_with(t))
}

fn tool_result_text(content: Option<&Value>) -> Option<String> {
    let mut out = String::new();
    match content? {
        Value::String(s) => push_capped(&mut out, s),
        Value::Array(items) => {
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(t) = item.get("text").and_then(Value::as_str)
                {
                    if !out.is_empty() {
                        push_capped(&mut out, "\n");
                    }
                    push_capped(&mut out, t);
                }
            }
        }
        _ => return None,
    }
    (!out.is_empty()).then_some(out)
}

fn push_capped(out: &mut String, s: &str) {
    let room = MAX_OUTPUT_SCAN.saturating_sub(out.len());
    if room == 0 {
        return;
    }
    if s.len() <= room {
        out.push_str(s);
    } else {
        let mut end = room;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&s[..end]);
    }
}

fn read_assistant(v: &Value, line: &mut Line) {
    let Some(items) = v.pointer("/message/content").and_then(Value::as_array) else {
        return;
    };
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
        let input = item.get("input");
        if WRITE_TOOLS.contains(&name) {
            let path = input
                .and_then(|i| i.get("file_path").or_else(|| i.get("notebook_path")))
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty());
            if let Some(p) = path {
                line.writes.push((name.to_string(), p.to_string()));
            }
        } else if name == "Bash"
            && let Some(cmd) = input.and_then(|i| i.get("command")).and_then(Value::as_str)
        {
            line.commands.push(cmd.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(json: &str) -> Line {
        match parse_line(json.as_bytes()) {
            Parsed::Line(l) => *l,
            other => panic!("expected a line, got {other:?}"),
        }
    }

    #[test]
    fn reads_common_fields() {
        let l = line(
            r#"{"type":"user","sessionId":"s1","cwd":"/w/shop","gitBranch":"feat/x",
                "timestamp":"2026-01-01T00:00:01.500Z","message":{"role":"user","content":"hello"}}"#,
        );
        assert_eq!(l.session_id.as_deref(), Some("s1"));
        assert_eq!(l.cwd.as_deref(), Some("/w/shop"));
        assert_eq!(l.git_branch.as_deref(), Some("feat/x"));
        assert_eq!(l.timestamp, Some(1_767_225_601_500));
        assert_eq!(l.prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn reads_titles() {
        assert_eq!(
            line(r#"{"type":"custom-title","customTitle":"Mine","sessionId":"s"}"#).custom_title,
            Some("Mine".into())
        );
        assert_eq!(
            line(r#"{"type":"ai-title","aiTitle":"Fix retries","sessionId":"s"}"#).ai_title,
            Some("Fix retries".into())
        );
    }

    #[test]
    fn prompt_from_text_items() {
        let l = line(
            r#"{"type":"user","message":{"content":[{"type":"text","text":"part one"},{"type":"image"},{"type":"text","text":"part two"}]}}"#,
        );
        assert_eq!(l.prompt.as_deref(), Some("part one\npart two"));
    }

    #[test]
    fn tool_results_are_outputs_not_prompts() {
        let l = line(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"wrote /w/out.html"}]}}"#,
        );
        assert_eq!(l.prompt, None);
        assert_eq!(l.outputs, vec!["wrote /w/out.html"]);
        let l = line(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"a"},{"type":"image"},{"type":"text","text":"b"}]}]}}"#,
        );
        assert_eq!(l.outputs, vec!["a\nb"]);
    }

    #[test]
    fn injected_and_command_lines_are_not_prompts() {
        for json in [
            r#"{"type":"user","isMeta":true,"message":{"content":"Caveat: injected"}}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"content":"summary"}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"subagent task"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/rename</command-name>"}}"#,
            r#"{"type":"user","message":{"content":"<local-command-stdout>ok</local-command-stdout>"}}"#,
            r#"{"type":"user","message":{"content":"<bash-input>ls</bash-input>"}}"#,
            r#"{"type":"user","message":{"content":"   "}}"#,
        ] {
            assert_eq!(line(json).prompt, None, "{json}");
        }
    }

    #[test]
    fn interrupts_are_noticed_and_are_not_prompts() {
        for text in [
            r#""[Request interrupted by user]""#,
            r#"[{"type":"text","text":"[Request interrupted by user for tool use]"}]"#,
        ] {
            let l = line(&format!(
                r#"{{"type":"user","message":{{"content":{text}}}}}"#
            ));
            assert!(l.interrupted, "{text}");
            assert_eq!(l.prompt, None);
        }
        assert!(!line(r#"{"type":"user","message":{"content":"carry on"}}"#).interrupted);
    }

    #[test]
    fn reads_write_tools_and_bash() {
        let l = line(
            r#"{"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":"..."},
                {"type":"tool_use","name":"Write","input":{"file_path":"/w/a.md","content":"x"}},
                {"type":"tool_use","name":"Edit","input":{"file_path":"/w/b.rs","old_string":"a","new_string":"b"}},
                {"type":"tool_use","name":"MultiEdit","input":{"file_path":"/w/c.rs","edits":[]}},
                {"type":"tool_use","name":"NotebookEdit","input":{"notebook_path":"/w/d.ipynb"}},
                {"type":"tool_use","name":"Read","input":{"file_path":"/w/e.rs"}},
                {"type":"tool_use","name":"Bash","input":{"command":"python make.py > out/r.html"}}
            ]}}"#,
        );
        assert_eq!(
            l.writes,
            vec![
                ("Write".into(), "/w/a.md".into()),
                ("Edit".into(), "/w/b.rs".into()),
                ("MultiEdit".into(), "/w/c.rs".into()),
                ("NotebookEdit".into(), "/w/d.ipynb".into()),
            ]
        );
        assert_eq!(l.commands, vec!["python make.py > out/r.html"]);
    }

    #[test]
    fn unknown_types_are_ignored_and_bad_json_is_malformed() {
        assert_eq!(
            parse_line(br#"{"type":"queue-operation"}"#),
            Parsed::Ignored
        );
        assert_eq!(parse_line(br#"{"type":"user""#), Parsed::Malformed);
        assert_eq!(parse_line(b"[1,2]"), Parsed::Malformed);
    }

    #[test]
    fn missing_fields_are_fine() {
        let l = line(r#"{"type":"assistant"}"#);
        assert_eq!(l, Line::default());
        let l = line(r#"{"type":"user","timestamp":"not a time","message":{"content":42}}"#);
        assert_eq!(l.timestamp, None);
        assert_eq!(l.prompt, None);
    }

    #[test]
    fn outputs_are_capped() {
        let big = "x".repeat(MAX_OUTPUT_SCAN + 10);
        let json = serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "content": big}]}
        });
        let l = line(&json.to_string());
        assert_eq!(l.outputs[0].len(), MAX_OUTPUT_SCAN);
    }
}
