//! The journal of MCP calls (PLAN.md, section 11), kept in memory for MailALM to show: when,
//! which tool, which accounts, how it ended, and whether the user confirmed it.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalog::Tool;
use crate::wire::ErrorCode;

/// Longest detail kept, in characters (a sent message's text is cut).
pub const MAX_DETAIL: usize = 500;

/// How a call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error(ErrorCode),
    /// The connection stopped waiting before it was run: nothing was done.
    Expired,
}

/// Whether the user was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// A read or a safe write.
    NotNeeded,
    /// The user confirmed it.
    Given,
    /// The user said no.
    Declined,
}

/// One call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    /// UTC Unix seconds.
    pub time: i64,
    pub tool: Tool,
    pub accounts: Vec<u32>,
    pub outcome: Outcome,
    pub confirmation: Confirmation,
    /// What was asked ([`crate::Call::describe`]), cut at [`MAX_DETAIL`] characters.
    pub detail: String,
}

impl JournalEntry {
    /// An entry dated now.
    pub fn now(tool: Tool, accounts: Vec<u32>, outcome: Outcome, confirmation: Confirmation, detail: String) -> JournalEntry {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let detail = match detail.char_indices().nth(MAX_DETAIL) {
            Some((cut, _)) => format!("{}…", &detail[..cut]),
            None => detail,
        };
        JournalEntry { time, tool, accounts, outcome, confirmation, detail }
    }
}

/// The last calls, oldest first, at most `cap` of them.
#[derive(Clone, Debug)]
pub struct Journal {
    entries: VecDeque<JournalEntry>,
    cap: usize,
}

impl Journal {
    /// Entries kept by [`Journal::default`].
    pub const DEFAULT_CAP: usize = 1000;

    /// A journal that keeps the last `cap` calls (at least one).
    pub fn new(cap: usize) -> Journal {
        let cap = cap.max(1);
        Journal { entries: VecDeque::with_capacity(cap.min(Self::DEFAULT_CAP)), cap }
    }

    /// Add a call; the oldest goes when the journal is full.
    pub fn record(&mut self, entry: JournalEntry) {
        while self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// The calls, oldest first (`.rev()` for the newest first).
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &JournalEntry> + ExactSizeIterator {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for Journal {
    fn default() -> Journal {
        Journal::new(Journal::DEFAULT_CAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u32) -> JournalEntry {
        JournalEntry::now(Tool::Search, vec![n], Outcome::Ok, Confirmation::NotNeeded, format!("call {n}"))
    }

    #[test]
    fn keeps_the_last_calls() {
        let mut j = Journal::new(3);
        assert!(j.is_empty());
        for n in 0..5 {
            j.record(entry(n));
        }
        assert_eq!(j.len(), 3);
        let kept: Vec<u32> = j.entries().map(|e| e.accounts[0]).collect();
        assert_eq!(kept, vec![2, 3, 4]);
        assert_eq!(j.entries().next_back().map(|e| e.detail.as_str()), Some("call 4"));
        assert!(j.entries().all(|e| e.time > 0));
        j.clear();
        assert!(j.is_empty());
        assert_eq!(Journal::new(0).cap, 1);
    }

    #[test]
    fn long_details_are_cut() {
        let e = JournalEntry::now(Tool::SendMessage, vec![1], Outcome::Ok, Confirmation::Given, "é".repeat(MAX_DETAIL + 10));
        assert_eq!(e.detail.chars().count(), MAX_DETAIL + 1);
        assert!(e.detail.ends_with('…'));
    }
}
