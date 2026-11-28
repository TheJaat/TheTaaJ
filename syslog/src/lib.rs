//! The system log (TheTaaJ K136): from the kernel's ring to the lines of
//! the log file, and back for the `log` command.
//!
//! The kernel copies everything written to the debug log into a ring
//! (`abi::log`), a piece at a time, each with who wrote it and when. A
//! piece is not a line: a program prints `argc `, then `1`, then the
//! newline, and other writers' pieces come in between. The log service
//! (`logd`) reads the ring and gives each piece to `Lines`, which keeps
//! one open line per writer and hands back each line once it ends - with
//! the time it began, the writer's name and a level.
//!
//! The file has one line per line, in a fixed form that `parse` reads
//! back:
//!
//! ```text
//! 2026-11-28 12:34:56.789 kernel     info  [kernel] idle
//! 2026-11-28 12:34:57.012 ahci       error [ahci] the controller would not start: no disk is attached
//! ```
//!
//! The level is the log service's reading of the text: programs do not
//! say one yet. What stops the system is `crit`; what failed, `error`;
//! what went wrong and was dealt with, `warn`; the rest, `info`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// How bad a line is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
    Crit,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Crit => "crit",
        }
    }

    pub fn parse(name: &str) -> Option<Level> {
        [Level::Info, Level::Warn, Level::Error, Level::Crit].into_iter().find(|level| level.name() == name)
    }
}

/// What the system's own messages say when something went wrong, from
/// the worst down. Matched without regard to case.
const CRIT: &[&str] = &["kernel panic", "cpu exception", "the system has stopped", "panicked at"];
const ERROR: &[&str] = &["fail", "could not", "cannot", "giving up", "error", "would not"];
const WARN: &[&str] = &["restarting", "missing", "not answered", "has not answered", "dropped", "warning", "lost", "did not", "no driver"];

/// The level of a line, read from its text.
pub fn level(text: &str) -> Level {
    let lower = text.to_ascii_lowercase();
    let any = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if any(CRIT) {
        Level::Crit
    } else if any(ERROR) {
        Level::Error
    } else if any(WARN) {
        Level::Warn
    } else {
        Level::Info
    }
}

/// Who wrote a piece.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Kernel,
    Program(u32),
    /// The boot before this one: the last words its kernel left in the
    /// crash record (K188). Every line of them is `crit`.
    Crash,
}

/// A whole line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    /// When its first piece was written: nanoseconds since 1970, 0 if
    /// before the kernel's clock ran.
    pub time: u64,
    /// "kernel", the program's name, or "pid N" if it was not seen
    /// starting.
    pub source: String,
    pub level: Level,
    pub text: String,
}

/// The longest line kept whole: a longer one is cut there.
pub const LINE_MAX: usize = 1024;
/// How many writers may have a line open at once; and how many
/// programs' names are kept.
const OPEN_MAX: usize = 64;
const NAMES_MAX: usize = 256;

struct Open {
    source: Source,
    time: u64,
    /// When it was last added to (`Lines::now`).
    touched: u64,
    text: Vec<u8>,
}

/// Each writer's open line, and the programs' names.
pub struct Lines {
    open: Vec<Open>,
    names: Vec<(u32, String)>,
    /// The latest time any piece had: what `flush_older` compares with.
    now: u64,
}

impl Default for Lines {
    fn default() -> Lines {
        Lines::new()
    }
}

impl Lines {
    pub fn new() -> Lines {
        Lines { open: Vec::new(), names: Vec::new(), now: 0 }
    }

    /// Program `pid` started, called `name`.
    pub fn started(&mut self, pid: u32, name: &str) {
        self.names.retain(|(p, _)| *p != pid);
        if self.names.len() == NAMES_MAX {
            self.names.remove(0);
        }
        self.names.push((pid, name.to_string()));
    }

    fn name(&self, source: Source) -> String {
        match source {
            Source::Kernel => "kernel".to_string(),
            Source::Crash => "crash".to_string(),
            Source::Program(pid) => match self.names.iter().rev().find(|(p, _)| *p == pid) {
                Some((_, name)) => name.clone(),
                None => format!("pid {pid}"),
            },
        }
    }

    fn line(&self, source: Source, time: u64, text: &[u8]) -> Line {
        // Kept to what reads in a file: no carriage returns, no other
        // control characters; text that is not UTF-8 shown as it can be.
        let text: String = String::from_utf8_lossy(text)
            .chars()
            .filter(|&c| c != '\r')
            .map(|c| if c.is_control() && c != '\t' { '?' } else { c })
            .collect();
        let level = if source == Source::Crash { Level::Crit } else { level(&text) };
        Line { time, source: self.name(source), level, text }
    }

    /// A piece `source` wrote at `time`; each line it ends goes to `out`.
    pub fn piece(&mut self, source: Source, time: u64, bytes: &[u8], out: &mut Vec<Line>) {
        if time > self.now {
            self.now = time;
        }
        let now = self.now;
        let mut index = self.open.iter().position(|open| open.source == source);
        for part in bytes.split_inclusive(|&b| b == b'\n') {
            let ends = part.last() == Some(&b'\n');
            let mut text = if ends { &part[..part.len() - 1] } else { part };
            // A line longer than LINE_MAX: cut there, and the rest begins
            // the next.
            loop {
                let at = match index {
                    Some(at) => at,
                    None => {
                        if self.open.len() == OPEN_MAX {
                            // Too many open lines: the one left longest goes.
                            let oldest = (0..self.open.len()).min_by_key(|&n| self.open[n].touched).unwrap_or(0);
                            let open = self.open.remove(oldest);
                            out.push(self.line(open.source, open.time, &open.text));
                        }
                        self.open.push(Open { source, time, touched: now, text: Vec::new() });
                        self.open.len() - 1
                    }
                };
                let open = &mut self.open[at];
                if open.time == 0 {
                    open.time = time;
                }
                open.touched = now;
                let room = LINE_MAX - open.text.len();
                let taken = text.len().min(room);
                open.text.extend_from_slice(&text[..taken]);
                text = &text[taken..];
                let full = open.text.len() == LINE_MAX;
                if full || (ends && text.is_empty()) {
                    let open = self.open.remove(at);
                    // The last words' blank lines say nothing.
                    if !(source == Source::Crash && open.text.is_empty()) {
                        out.push(self.line(open.source, open.time, &open.text));
                    }
                    index = None;
                } else {
                    index = Some(at);
                }
                if text.is_empty() {
                    break;
                }
            }
        }
    }

    /// Lines left open for longer than `age` nanoseconds, as they are -
    /// a prompt waits for its Enter for ever, and the log must not.
    pub fn flush_older(&mut self, age: u64, out: &mut Vec<Line>) {
        let now = self.now;
        let mut n = 0;
        while n < self.open.len() {
            if now.saturating_sub(self.open[n].touched) > age && !self.open[n].text.is_empty() {
                let open = self.open.remove(n);
                out.push(self.line(open.source, open.time, &open.text));
            } else {
                n += 1;
            }
        }
    }
}

/// The width of `date`'s answer.
pub const DATE_WIDTH: usize = 23;

/// `ns` since 1970 as "YYYY-MM-DD HH:MM:SS.mmm" (UTC); 0 as "early boot",
/// padded to the same width.
pub fn date(ns: u64) -> String {
    if ns == 0 {
        return format!("{:<width$}", "early boot", width = DATE_WIDTH);
    }
    let seconds = ns / 1_000_000_000;
    let millis = (ns / 1_000_000) % 1000;
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = civil(days);
    let in_day = seconds % 86_400;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}", in_day / 3600, in_day / 60 % 60, in_day % 60)
}

/// Days since 1970-01-01 as a year, month and day (Howard Hinnant's
/// `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The width of the source's column; a longer name is not cut.
pub const SOURCE_WIDTH: usize = 10;

/// A line as the file has it, with its newline.
pub fn format(line: &Line) -> String {
    format!("{} {:<sw$} {:<5} {}\n", date(line.time), line.source, line.level.name(), line.text, sw = SOURCE_WIDTH)
}

/// A line of the file, read back: its date as written, its source, its
/// level and its text. None if it is not one.
pub fn parse(line: &str) -> Option<(&str, &str, Level, &str)> {
    let date = line.get(..DATE_WIDTH)?;
    let rest = line.get(DATE_WIDTH..)?.strip_prefix(' ')?;
    let rest = rest.trim_start_matches(' ');
    // The source may be "pid 7": two words, the second a number.
    let (source, rest) = match rest.strip_prefix("pid ") {
        Some(after) => {
            let digits = after.find(|c: char| !c.is_ascii_digit())?;
            (&rest[..4 + digits], &after[digits..])
        }
        None => rest.split_at(rest.find(' ')?),
    };
    let rest = rest.trim_start_matches(' ');
    let (level, rest) = rest.split_at(rest.find(' ').unwrap_or(rest.len()));
    let level = Level::parse(level)?;
    let text = rest.strip_prefix(' ').unwrap_or(rest).trim_start_matches(' ');
    let text = text.strip_suffix('\n').unwrap_or(text);
    Some((date.trim_end(), source, level, text))
}

#[cfg(test)]
mod tests;
