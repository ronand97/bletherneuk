//! Live chat status from Claude Code hook events (design §12).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A chat that has been "working" this long with no events, and whose iTerm2 session is
/// gone, is marked closed.
pub const STALE_MS: i64 = 10 * 60 * 1000;

/// How much prompt text a hook event carries to the service.
pub const MAX_PROMPT_CHARS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idle,
    Working,
    NeedsYou,
    Done,
    Closed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Working => "working",
            Status::NeedsYou => "needs_you",
            Status::Done => "done",
            Status::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "working" => Status::Working,
            "needs_you" => Status::NeedsYou,
            "done" => Status::Done,
            "closed" => Status::Closed,
            _ => Status::Idle,
        }
    }

    /// Anything but working counts as stopped, for keep-awake.
    pub fn is_stopped(self) -> bool {
        self != Status::Working
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub status: Status,
    /// Working through background tasks or subagents after the turn ended.
    pub background: bool,
}

/// The parts of a hook event the service uses. The hook strips everything else.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Paths a write tool is about to touch or touched (Write/Edit/MultiEdit/NotebookEdit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// How many background tasks were still running (Stop and SubagentStop only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tasks: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Event {
    /// Picks the fields bletherneuk uses out of a raw hook payload.
    pub fn from_payload(v: &Value) -> Option<Event> {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let name = s("hook_event_name")?;
        let session_id = s("session_id")?;
        let tool_name = s("tool_name");
        let file_path = match tool_name.as_deref() {
            Some("Write" | "Edit" | "MultiEdit" | "NotebookEdit") => v
                .get("tool_input")
                .and_then(|i| i.get("file_path").or_else(|| i.get("notebook_path")))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        Some(Event {
            name,
            session_id,
            transcript_path: s("transcript_path"),
            cwd: s("cwd"),
            prompt: s("prompt").map(|p| crate::util::truncate_chars(&p, MAX_PROMPT_CHARS)),
            notification_type: s("notification_type"),
            tool_name,
            file_path,
            agent_id: s("agent_id"),
            background_tasks: v
                .get("background_tasks")
                .and_then(Value::as_array)
                .map(|a| a.len()),
            source: s("source"),
            reason: s("reason"),
        })
    }
}

/// Notification types that mean Claude is waiting on you.
const NEEDS_YOU_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// The next state after an event, or `None` if the event changes nothing.
pub fn transition(prev: Option<State>, ev: &Event) -> Option<State> {
    let prev_status = prev.map(|p| p.status);
    let working = |background: bool| State {
        status: Status::Working,
        background,
    };
    let stopped = |status| State {
        status,
        background: false,
    };
    let next = match ev.name.as_str() {
        "SessionStart" => stopped(Status::Idle),
        "UserPromptSubmit" => working(false),
        "PreToolUse" if ev.tool_name.as_deref() == Some("AskUserQuestion") => {
            stopped(Status::NeedsYou)
        }
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "SubagentStart" => {
            // Tool use after the turn ended is background work (a subagent or a task).
            let background = match prev {
                Some(p) if p.status == Status::Working => p.background,
                Some(p) => matches!(p.status, Status::Done | Status::Idle) && ev.agent_id.is_some(),
                None => false,
            };
            working(background)
        }
        "PermissionRequest" => stopped(Status::NeedsYou),
        "Notification" => match ev.notification_type.as_deref() {
            Some(t) if NEEDS_YOU_NOTIFICATIONS.contains(&t) => stopped(Status::NeedsYou),
            // Claude is idle at the prompt: whatever background work there was is over.
            Some("idle_prompt")
                if prev.is_some_and(|p| p.status == Status::Working && p.background) =>
            {
                stopped(Status::Done)
            }
            Some(_) => return None,
            // Older versions sent no type; their notifications were permission prompts or
            // the idle reminder, and only the first means "needs you".
            None if prev_status == Some(Status::Working) => stopped(Status::NeedsYou),
            None => return None,
        },
        "Stop" => match ev.background_tasks {
            Some(n) if n > 0 => working(true),
            _ => stopped(Status::Done),
        },
        "StopFailure" => stopped(Status::Done),
        "SubagentStop" => match prev {
            Some(p)
                if p.status == Status::Working
                    && p.background
                    && ev.background_tasks == Some(0) =>
            {
                stopped(Status::Done)
            }
            _ => return None,
        },
        "SessionEnd" => stopped(Status::Closed),
        _ => return None,
    };
    (Some(next) != prev).then_some(next)
}

/// You focused the chat's tab: a finished chat becomes idle.
pub fn on_focus(prev: State) -> Option<State> {
    (prev.status == Status::Done).then_some(State {
        status: Status::Idle,
        background: false,
    })
}

/// A "working" chat with no events for [`STALE_MS`] whose iTerm2 session is gone.
pub fn is_stale(state: State, last_event_ms: i64, now_ms: i64, session_exists: bool) -> bool {
    state.status == Status::Working && now_ms - last_event_ms >= STALE_MS && !session_exists
}

/// Does this event start or end a turn?
#[derive(Debug, PartialEq, Eq)]
pub enum TurnEdge {
    Start,
    End,
    None,
}

pub fn turn_edge(ev: &Event) -> TurnEdge {
    match ev.name.as_str() {
        "UserPromptSubmit" => TurnEdge::Start,
        "Stop" | "StopFailure" | "SessionEnd" => TurnEdge::End,
        _ => TurnEdge::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(name: &str) -> Event {
        Event {
            name: name.into(),
            session_id: "s".into(),
            ..Event::default()
        }
    }

    fn st(status: Status, background: bool) -> Option<State> {
        Some(State { status, background })
    }

    #[test]
    fn the_basic_cycle() {
        let s0 = transition(None, &ev("SessionStart"));
        assert_eq!(s0, st(Status::Idle, false));
        let s1 = transition(s0, &ev("UserPromptSubmit"));
        assert_eq!(s1, st(Status::Working, false));
        assert_eq!(transition(s1, &ev("PreToolUse")), None, "no change");
        let s2 = transition(s1, &ev("Stop"));
        assert_eq!(s2, st(Status::Done, false));
        assert_eq!(
            on_focus(s2.unwrap()),
            Some(State {
                status: Status::Idle,
                background: false
            })
        );
        assert_eq!(transition(s2, &ev("SessionEnd")), st(Status::Closed, false));
    }

    #[test]
    fn permission_and_questions_need_you() {
        let working = st(Status::Working, false);
        assert_eq!(
            transition(working, &ev("PermissionRequest")),
            st(Status::NeedsYou, false)
        );
        let ask = Event {
            tool_name: Some("AskUserQuestion".into()),
            ..ev("PreToolUse")
        };
        assert_eq!(transition(working, &ask), st(Status::NeedsYou, false));
        for t in [
            "permission_prompt",
            "elicitation_dialog",
            "agent_needs_input",
        ] {
            let n = Event {
                notification_type: Some(t.into()),
                ..ev("Notification")
            };
            assert_eq!(transition(working, &n), st(Status::NeedsYou, false), "{t}");
        }
        let auth = Event {
            notification_type: Some("auth_success".into()),
            ..ev("Notification")
        };
        assert_eq!(transition(working, &auth), None);
        // After you answer, the next tool event means working again.
        assert_eq!(
            transition(st(Status::NeedsYou, false), &ev("PostToolUse")),
            st(Status::Working, false)
        );
    }

    #[test]
    fn untyped_notifications_from_older_versions() {
        assert_eq!(
            transition(st(Status::Working, false), &ev("Notification")),
            st(Status::NeedsYou, false)
        );
        assert_eq!(
            transition(st(Status::Done, false), &ev("Notification")),
            None
        );
    }

    #[test]
    fn background_work_keeps_a_chat_working() {
        let working = st(Status::Working, false);
        let stop = Event {
            background_tasks: Some(2),
            ..ev("Stop")
        };
        let bg = transition(working, &stop);
        assert_eq!(bg, st(Status::Working, true));
        // A subagent finishing while others still run changes nothing.
        let sub = Event {
            background_tasks: Some(1),
            ..ev("SubagentStop")
        };
        assert_eq!(transition(bg, &sub), None);
        let last = Event {
            background_tasks: Some(0),
            ..ev("SubagentStop")
        };
        assert_eq!(transition(bg, &last), st(Status::Done, false));
        let idle = Event {
            notification_type: Some("idle_prompt".into()),
            ..ev("Notification")
        };
        assert_eq!(transition(bg, &idle), st(Status::Done, false));
        assert_eq!(transition(st(Status::Done, false), &idle), None);
    }

    #[test]
    fn subagent_tool_use_after_a_turn_is_background_work() {
        let t = Event {
            agent_id: Some("a1".into()),
            ..ev("PreToolUse")
        };
        assert_eq!(
            transition(st(Status::Done, false), &t),
            st(Status::Working, true)
        );
        assert_eq!(
            transition(st(Status::Done, false), &ev("PreToolUse")),
            st(Status::Working, false)
        );
    }

    #[test]
    fn failures_end_the_turn() {
        assert_eq!(
            transition(st(Status::Working, false), &ev("StopFailure")),
            st(Status::Done, false)
        );
        assert_eq!(transition(None, &ev("SomethingNew")), None);
    }

    #[test]
    fn stale_working_chats() {
        let w = State {
            status: Status::Working,
            background: false,
        };
        assert!(is_stale(w, 0, STALE_MS, false));
        assert!(!is_stale(w, 0, STALE_MS, true));
        assert!(!is_stale(w, 0, STALE_MS - 1, false));
        let d = State {
            status: Status::Done,
            background: false,
        };
        assert!(!is_stale(d, 0, STALE_MS, false));
    }

    #[test]
    fn payloads_are_reduced_to_what_is_used() {
        let v = json!({
            "hook_event_name": "PreToolUse", "session_id": "s1", "cwd": "/w",
            "transcript_path": "/h/t.jsonl", "tool_name": "Write",
            "tool_input": {"file_path": "/w/a.md", "content": "big secret body"},
            "permission_mode": "default"
        });
        let e = Event::from_payload(&v).unwrap();
        assert_eq!(e.file_path.as_deref(), Some("/w/a.md"));
        let back = serde_json::to_string(&e).unwrap();
        assert!(!back.contains("big secret body"));

        let stop = json!({"hook_event_name": "Stop", "session_id": "s1",
                          "background_tasks": [{"id": "b1", "type": "shell"}],
                          "last_assistant_message": "long reply"});
        let e = Event::from_payload(&stop).unwrap();
        assert_eq!(e.background_tasks, Some(1));
        assert!(!serde_json::to_string(&e).unwrap().contains("long reply"));

        let prompt = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1",
                            "prompt": "p".repeat(5000)});
        assert_eq!(
            Event::from_payload(&prompt).unwrap().prompt.unwrap().len(),
            MAX_PROMPT_CHARS
        );
        assert_eq!(Event::from_payload(&json!({"session_id": "s"})), None);
    }

    #[test]
    fn turn_edges() {
        assert_eq!(turn_edge(&ev("UserPromptSubmit")), TurnEdge::Start);
        assert_eq!(turn_edge(&ev("Stop")), TurnEdge::End);
        assert_eq!(turn_edge(&ev("SessionEnd")), TurnEdge::End);
        assert_eq!(turn_edge(&ev("PreToolUse")), TurnEdge::None);
    }
}
