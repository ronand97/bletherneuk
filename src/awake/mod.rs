//! "Keep the Mac awake until this chat is done" (design §12).
//!
//! One `/usr/bin/caffeinate -i -w <service pid>` runs while any request is active.
//! Because of `-w`, caffeinate exits if the service dies, so the Mac can't get stuck awake.

use std::collections::{BTreeSet, HashMap};

use crate::status::Status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: i64,
    pub chat_id: Option<String>,
    pub block_id: Option<i64>,
    pub chats: BTreeSet<String>,
    pub started_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Done,
    MaxHours,
    Cancelled,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::Done => "done",
            EndReason::MaxHours => "max_hours",
            EndReason::Cancelled => "cancelled",
        }
    }
}

/// Starts and stops the `caffeinate` process. Tests use a fake.
pub trait Launcher: Send {
    fn start(&mut self) -> std::io::Result<()>;
    fn stop(&mut self);
    fn running(&mut self) -> bool;
}

pub struct Manager<L: Launcher> {
    launcher: L,
    grace_ms: i64,
    max_ms: i64,
    active: Vec<Request>,
    /// When each chat was first seen stopped (and has stayed stopped since).
    stopped_since: HashMap<String, i64>,
}

impl<L: Launcher> Manager<L> {
    pub fn new(launcher: L, grace_seconds: u64, max_hours: u64) -> Self {
        Manager {
            launcher,
            grace_ms: grace_seconds as i64 * 1000,
            max_ms: max_hours as i64 * 3_600_000,
            active: Vec::new(),
            stopped_since: HashMap::new(),
        }
    }

    pub fn set_limits(&mut self, grace_seconds: u64, max_hours: u64) {
        self.grace_ms = grace_seconds as i64 * 1000;
        self.max_ms = max_hours as i64 * 3_600_000;
    }

    /// Adds a request (new, or restored after a restart).
    pub fn add(&mut self, r: Request) -> std::io::Result<()> {
        self.active.retain(|a| a.id != r.id);
        self.active.push(r);
        self.sync()
    }

    pub fn cancel(&mut self, id: i64) -> Option<(Request, EndReason)> {
        let pos = self.active.iter().position(|r| r.id == id)?;
        let r = self.active.remove(pos);
        let _ = self.sync();
        Some((r, EndReason::Cancelled))
    }

    pub fn active(&self) -> &[Request] {
        &self.active
    }

    /// Chats covered by at least one active request.
    pub fn covered_chats(&self) -> BTreeSet<String> {
        self.active
            .iter()
            .flat_map(|r| r.chats.iter().cloned())
            .collect()
    }

    pub fn is_holding(&mut self) -> bool {
        self.launcher.running()
    }

    /// Updates timers from current statuses and ends requests that are finished.
    /// A chat counts as done once it has stayed stopped for the grace period; a missing
    /// chat counts as stopped.
    pub fn tick(
        &mut self,
        statuses: &HashMap<String, Status>,
        now: i64,
    ) -> Vec<(Request, EndReason)> {
        for chat in self.covered_chats() {
            let stopped = statuses.get(&chat).is_none_or(|s| s.is_stopped());
            if stopped {
                self.stopped_since.entry(chat).or_insert(now);
            } else {
                self.stopped_since.remove(&chat);
            }
        }
        let mut ended = Vec::new();
        let grace = self.grace_ms;
        let max = self.max_ms;
        let stopped_since = &self.stopped_since;
        self.active.retain(|r| {
            if now - r.started_at >= max {
                ended.push((r.clone(), EndReason::MaxHours));
                return false;
            }
            let all_done = r
                .chats
                .iter()
                .all(|c| stopped_since.get(c).is_some_and(|t| now - t >= grace));
            if all_done {
                ended.push((r.clone(), EndReason::Done));
                return false;
            }
            true
        });
        let covered = self.covered_chats();
        self.stopped_since.retain(|c, _| covered.contains(c));
        let _ = self.sync();
        ended
    }

    /// caffeinate runs exactly while some request is active. A caffeinate that died is
    /// started again.
    fn sync(&mut self) -> std::io::Result<()> {
        if self.active.is_empty() {
            if self.launcher.running() {
                self.launcher.stop();
            }
            Ok(())
        } else if !self.launcher.running() {
            self.launcher.start()
        } else {
            Ok(())
        }
    }
}

/// The real launcher: `/usr/bin/caffeinate -i -w <pid>`, arguments passed as a list.
pub struct Caffeinate {
    pid: u32,
    child: Option<std::process::Child>,
}

impl Caffeinate {
    pub fn new(service_pid: u32) -> Caffeinate {
        Caffeinate {
            pid: service_pid,
            child: None,
        }
    }
}

impl Launcher for Caffeinate {
    fn start(&mut self) -> std::io::Result<()> {
        let child = std::process::Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &self.pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        self.child = Some(child);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn running(&mut self) -> bool {
        match &mut self.child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }
}

impl Drop for Caffeinate {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fake {
        running: Arc<Mutex<bool>>,
        starts: Arc<Mutex<u32>>,
    }

    impl Launcher for Fake {
        fn start(&mut self) -> std::io::Result<()> {
            *self.running.lock().unwrap() = true;
            *self.starts.lock().unwrap() += 1;
            Ok(())
        }
        fn stop(&mut self) {
            *self.running.lock().unwrap() = false;
        }
        fn running(&mut self) -> bool {
            *self.running.lock().unwrap()
        }
    }

    const GRACE_S: u64 = 120;
    const MAX_H: u64 = 12;
    const MIN: i64 = 60_000;

    fn req(id: i64, chats: &[&str], at: i64) -> Request {
        Request {
            id,
            chat_id: (chats.len() == 1).then(|| chats[0].to_string()),
            block_id: None,
            chats: chats.iter().map(|c| c.to_string()).collect(),
            started_at: at,
        }
    }

    fn statuses(pairs: &[(&str, Status)]) -> HashMap<String, Status> {
        pairs.iter().map(|(c, s)| (c.to_string(), *s)).collect()
    }

    #[test]
    fn runs_while_working_and_ends_after_the_grace_period() {
        let fake = Fake::default();
        let mut m = Manager::new(fake.clone(), GRACE_S, MAX_H);
        m.add(req(1, &["a"], 0)).unwrap();
        assert!(m.is_holding());

        assert!(m.tick(&statuses(&[("a", Status::Working)]), MIN).is_empty());
        // Stops at minute 2; still held through the grace period.
        assert!(
            m.tick(&statuses(&[("a", Status::Done)]), 2 * MIN)
                .is_empty()
        );
        assert!(
            m.tick(&statuses(&[("a", Status::Done)]), 3 * MIN)
                .is_empty()
        );
        assert!(m.is_holding());
        let ended = m.tick(&statuses(&[("a", Status::Done)]), 4 * MIN);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].1, EndReason::Done);
        assert!(!m.is_holding());
    }

    #[test]
    fn work_restarting_inside_the_grace_period_resets_it() {
        let mut m = Manager::new(Fake::default(), GRACE_S, MAX_H);
        m.add(req(1, &["a"], 0)).unwrap();
        m.tick(&statuses(&[("a", Status::Done)]), MIN);
        m.tick(&statuses(&[("a", Status::Working)]), 2 * MIN);
        assert!(
            m.tick(&statuses(&[("a", Status::NeedsYou)]), 3 * MIN)
                .is_empty()
        );
        assert!(
            m.tick(&statuses(&[("a", Status::NeedsYou)]), 4 * MIN)
                .is_empty()
        );
        assert_eq!(
            m.tick(&statuses(&[("a", Status::NeedsYou)]), 5 * MIN).len(),
            1
        );
    }

    #[test]
    fn block_requests_wait_for_every_chat() {
        let mut m = Manager::new(Fake::default(), GRACE_S, MAX_H);
        m.add(req(1, &["a", "b"], 0)).unwrap();
        m.tick(
            &statuses(&[("a", Status::Done), ("b", Status::Working)]),
            MIN,
        );
        assert!(
            m.tick(
                &statuses(&[("a", Status::Done), ("b", Status::Working)]),
                10 * MIN
            )
            .is_empty()
        );
        m.tick(
            &statuses(&[("a", Status::Done), ("b", Status::Closed)]),
            11 * MIN,
        );
        assert_eq!(
            m.tick(
                &statuses(&[("a", Status::Done), ("b", Status::Closed)]),
                13 * MIN
            )
            .len(),
            1
        );
    }

    #[test]
    fn requests_end_after_the_cap() {
        let mut m = Manager::new(Fake::default(), GRACE_S, MAX_H);
        m.add(req(1, &["a"], 0)).unwrap();
        let hour = 60 * MIN;
        assert!(
            m.tick(&statuses(&[("a", Status::Working)]), 11 * hour)
                .is_empty()
        );
        let ended = m.tick(&statuses(&[("a", Status::Working)]), 12 * hour);
        assert_eq!(ended[0].1, EndReason::MaxHours);
        assert!(!m.is_holding());
    }

    #[test]
    fn one_caffeinate_for_many_requests_and_restarted_if_it_dies() {
        let fake = Fake::default();
        let mut m = Manager::new(fake.clone(), GRACE_S, MAX_H);
        m.add(req(1, &["a"], 0)).unwrap();
        m.add(req(2, &["b"], 0)).unwrap();
        assert_eq!(*fake.starts.lock().unwrap(), 1);
        *fake.running.lock().unwrap() = false; // caffeinate was killed
        m.tick(
            &statuses(&[("a", Status::Working), ("b", Status::Working)]),
            MIN,
        );
        assert_eq!(*fake.starts.lock().unwrap(), 2);
        m.cancel(1);
        assert!(m.is_holding());
        m.cancel(2);
        assert!(!m.is_holding());
    }

    #[test]
    fn restored_requests_start_caffeinate_again() {
        let fake = Fake::default();
        let mut m = Manager::new(fake.clone(), GRACE_S, MAX_H);
        // As after a restart: an active request from the database.
        m.add(req(7, &["a"], -30 * MIN)).unwrap();
        assert!(m.is_holding());
        assert_eq!(m.covered_chats(), BTreeSet::from(["a".to_string()]));
    }

    #[test]
    fn real_caffeinate_follows_its_watched_process() {
        // Watch a short-lived child instead of the test process.
        let mut sleeper = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut c = Caffeinate::new(sleeper.id());
        c.start().unwrap();
        assert!(c.running());
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
        let mut gone = false;
        for _ in 0..50 {
            if !c.running() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(gone, "caffeinate -w exits when the watched process dies");
    }
}
