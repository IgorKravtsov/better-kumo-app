//! The tray app's log: where it lives, keeping it small, and a redacted tail for problem reports.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const ISSUES_NEW: &str = "https://github.com/IgorKravtsov/better-kumo-app/issues/new";
pub const REPORT_LINES: usize = 300;
/// Stands for the meetings folder in a report.
pub const MEETINGS: &str = "<meetings>";
/// The full log handed to a report, beside the log itself.
pub const EXPORT: &str = "better-kumo-log.txt";
/// GitHub refuses longer new-issue links.
const MAX_URL: usize = 7_000;
const MB: u64 = 1024 * 1024;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogsConfig {
    pub keep_days: u64,
    pub max_mb: u64,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            keep_days: 7,
            max_mb: 20,
        }
    }
}

impl LogsConfig {
    pub fn validated(self) -> anyhow::Result<Self> {
        if !(1..=365).contains(&self.keep_days) {
            anyhow::bail!("Keep logs must be 1 to 365 days");
        }
        if !(1..=1_000).contains(&self.max_mb) {
            anyhow::bail!("The log size limit must be 1 to 1000 MB");
        }
        Ok(self)
    }

    /// The current file moves aside past this, so one busy day cannot use the whole budget.
    fn part_bytes(&self) -> u64 {
        // A quarter even at the 1 MB minimum, so a moved part always fits the budget.
        self.max_bytes() / 4
    }

    fn max_bytes(&self) -> u64 {
        self.max_mb.saturating_mul(MB)
    }
}

pub fn log_path() -> PathBuf {
    let home = std::env::home_dir().unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/kumowatch/better-kumo.log")
    } else {
        home.join(".local/share/kumowatch-relay/better-kumo.log")
    }
}

fn day_of(time: SystemTime) -> String {
    jiff::Timestamp::try_from(time)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).date().to_string())
        .unwrap_or_default()
}

fn rotated(path: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Only names `rotate` gives: a copy saved by hand beside the log is not ours to delete.
    let Ok(ours) = Regex::new(&format!(
        r"^{}\.(?:\d{{4}}-\d{{2}}-\d{{2}})?(?:-\d+)?\.log$|^{}\.old\.log$",
        regex::escape(&stem),
        regex::escape(&stem)
    )) else {
        return Vec::new();
    };
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, std::fs::Metadata)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    ours.is_match(&name) && e.path() != path
                })
                // DirEntry metadata does not follow links: a linked file is skipped, not read or deleted.
                .filter_map(|e| Some((e.path(), e.metadata().ok()?)))
                .filter(|(_, meta)| meta.is_file())
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|(_, m)| std::cmp::Reverse(m.modified().ok()));
    files
}

/// True when the log moved aside and the caller must reopen it.
pub fn rotate(path: &Path, config: &LogsConfig, now: SystemTime) -> bool {
    let moved = std::fs::metadata(path).is_ok_and(|meta| {
        let born = meta.created().or_else(|_| meta.modified()).unwrap_or(now);
        let day = day_of(born);
        if day == day_of(now) && meta.len() <= config.part_bytes() {
            return false;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let target = (1..)
            .map(|n| {
                let suffix = if n == 1 {
                    String::new()
                } else {
                    format!("-{n}")
                };
                path.with_file_name(format!("{stem}.{day}{suffix}.log"))
            })
            .find(|p| !p.exists())
            .unwrap_or_else(|| path.with_extension("old.log"));
        std::fs::rename(path, target).is_ok()
    });
    prune(path, config, now);
    moved
}

fn prune(path: &Path, config: &LogsConfig, now: SystemTime) {
    // A report's copy is for attaching right away; it goes with the logs it was cut from.
    let export = path.with_file_name(EXPORT);
    if std::fs::metadata(&export)
        .and_then(|m| m.modified())
        .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > DAY))
    {
        let _ = std::fs::remove_file(export);
    }
    let oldest = now.checked_sub(Duration::from_secs(
        config.keep_days.saturating_mul(DAY.as_secs()),
    ));
    let mut used = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    for (file, meta) in rotated(path) {
        let expired = match (meta.modified(), oldest) {
            (Ok(modified), Some(oldest)) => modified < oldest,
            _ => false,
        };
        if expired || used + meta.len() > config.max_bytes() {
            let _ = std::fs::remove_file(file);
        } else {
            used += meta.len();
        }
    }
}

pub fn log_bytes(path: &Path) -> u64 {
    let current = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    current + rotated(path).iter().map(|(_, m)| m.len()).sum::<u64>()
}

pub fn tail(path: &Path, lines: usize) -> String {
    let mut parts = Vec::new();
    let mut have = 0;
    let files =
        std::iter::once(path.to_path_buf()).chain(rotated(path).into_iter().map(|(p, _)| p));
    for file in files {
        if have >= lines {
            break;
        }
        let text = tail_of(&file, lines - have);
        if !text.is_empty() {
            have += text.lines().count();
            parts.push(text);
        }
    }
    parts.reverse();
    parts.join("\n")
}

fn tail_of(path: &Path, lines: usize) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    // About 400 bytes a line is plenty; logs are mostly short status lines.
    let window = (lines as u64 * 400).max(4096);
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(window)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.trim_end().lines().collect();
    let skip_partial = usize::from(len > window);
    let from = all
        .len()
        .saturating_sub(lines)
        .max(skip_partial.min(all.len()));
    all[from..].join("\n")
}

#[derive(Default)]
pub struct Redactor {
    literals: Vec<(String, String)>,
    names: Option<Regex>,
    me: Option<String>,
}

impl Redactor {
    pub fn new(
        literals: Vec<(String, String)>,
        names: impl IntoIterator<Item = String>,
        me: Option<String>,
    ) -> Self {
        let mut names: Vec<String> = names
            .into_iter()
            .chain(me.clone())
            .map(|n| n.trim().to_string())
            .filter(|n| n.chars().count() >= 2)
            .collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        names.dedup();
        // Not \b: it never matches after a name that ends in punctuation, like "Amy (QA)".
        let names = (!names.is_empty())
            .then(|| {
                let alternation: Vec<String> = names.iter().map(|n| regex::escape(n)).collect();
                Regex::new(&alternation.join("|")).ok()
            })
            .flatten();
        let home = std::env::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .filter(|h| h.len() > 1);
        let mut literals: Vec<(String, String)> = literals
            .into_iter()
            .filter(|(from, _)| from.len() >= 3)
            .chain(home.map(|h| (h, "~".into())))
            .collect();
        literals.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        Self {
            literals,
            names,
            me: me.map(|m| m.trim().to_string()),
        }
    }

    pub fn apply(&self, text: &str) -> String {
        static EMAIL: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+").unwrap());
        static QUERY: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(https?://[^\s?#]+)[?#][^\s)]*").unwrap());
        static UUID: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
            )
            .unwrap()
        });
        static TOKEN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9_-]{20,}\b").unwrap());
        static IPV4: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b").unwrap()
        });

        // Alert titles hold names in many shapes (and chat text), and old lines name people the
        // directory may no longer list: the kind is enough for a report.
        static NOTICE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^(desktop: [A-Z]\w* \().*\)$").unwrap());

        // Covers a space renamed since, or a relay.toml that is missing or unreadable.
        static STARTUP: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^(kumo-relay: space )\S+ as [^\s,]+").unwrap());

        let mut text = text.to_string();
        for (from, to) in &self.literals {
            text = text.replace(from.as_str(), to);
        }
        if let Some(names) = &self.names {
            text = self.replace_names(names, &text);
        }
        static HOME: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(/Users/|/home/|\\Users\\)[^/\\\s]+").unwrap());
        text = STARTUP
            .replace_all(&text, "${1}<space> as <my-id>")
            .into_owned();
        text = NOTICE.replace_all(&text, "${1}<title>)").into_owned();
        // A meeting folder is named after its room: keep only that it was one.
        // Also under a folder used before the current one, by its default name.
        static MEETING: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(<meetings>|(?:[A-Za-z]:)?[/\\][^:\n]*?[/\\]Meetings)[/\\][^:\n]*")
                .unwrap()
        });
        text = MEETING
            .replace_all(&text, "<meetings>/<meeting>")
            .into_owned();
        // Lines from before ntfy errors kept only the status carry the server's reply.
        static NTFY: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^(ntfy: ntfy \d{3}[^:\n]*): .*$").unwrap());
        text = NTFY.replace_all(&text, "${1}: <reply>").into_owned();
        text = HOME.replace_all(&text, "${1}<user>").into_owned();
        text = EMAIL.replace_all(&text, "<email>").into_owned();
        text = QUERY.replace_all(&text, "$1?…").into_owned();
        text = UUID.replace_all(&text, "<uuid>").into_owned();
        text = TOKEN
            .replace_all(&text, |c: &regex::Captures| {
                let token = &c[0];
                // Ids mix digits in; long-hyphenated words like file names do not get eaten.
                if token.bytes().any(|b| b.is_ascii_digit())
                    && token.bytes().any(|b| b.is_ascii_alphabetic())
                {
                    "<id>".to_string()
                } else {
                    token.to_string()
                }
            })
            .into_owned();
        IPV4.replace_all(&text, |c: &regex::Captures| {
            let ip = &c[0];
            let private = ip.starts_with("127.") || ip == "0.0.0.0";
            if private {
                ip.to_string()
            } else {
                "<ip>".to_string()
            }
        })
        .into_owned()
    }
}

impl Redactor {
    fn replace_names(&self, names: &Regex, text: &str) -> String {
        let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        let mut at = 0;
        while let Some(m) = names.find_at(text, at) {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            if word(before) || word(after) {
                at = m.start() + text[m.start()..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            let name = m.as_str();
            let label = if self.me.as_deref() == Some(name) {
                "<me>".to_string()
            } else {
                let next = seen.len() + 1;
                seen.entry(name.to_string())
                    .or_insert_with(|| format!("<person {next}>"))
                    .clone()
            };
            out.push_str(&text[copied..m.start()]);
            out.push_str(&label);
            copied = m.end();
            at = m.end();
        }
        out.push_str(&text[copied..]);
        out
    }
}

/// Asks the system once: the version cannot change while the app runs.
pub fn os() -> String {
    static OS: LazyLock<String> = LazyLock::new(describe_os);
    OS.clone()
}

fn describe_os() -> String {
    let version = os_version().unwrap_or_default();
    let name = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    format!("{name} {version} ({})", std::env::consts::ARCH).replace("  ", " ")
}

fn os_version() -> Option<String> {
    let output = if cfg!(target_os = "macos") {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
    } else if cfg!(target_os = "linux") {
        std::process::Command::new("uname").arg("-r").output()
    } else {
        return None;
    };
    let text = String::from_utf8(output.ok()?.stdout).ok()?;
    Some(text.trim().to_string())
}

pub fn issue_url(environment: &str, log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let url = |shown: &[&str]| {
        format!(
            "{ISSUES_NEW}?title={}&body={}",
            encode("Problem: "),
            encode(&issue_body(environment, shown))
        )
    };
    // The newest lines that fit: each costs its encoded length plus an encoded newline, so one
    // long old line drops only itself and what is older.
    let mut used = url(&lines[lines.len().saturating_sub(1)..]).len();
    let mut from = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        let cost = if i + 1 == lines.len() {
            0
        } else {
            encode(line).len() + "%0A".len()
        };
        if used + cost > MAX_URL {
            break;
        }
        used += cost;
        from = i;
    }
    // The summary's line count can add a digit or two.
    loop {
        let full = url(&lines[from..]);
        if full.len() <= MAX_URL || from == lines.len() {
            return full;
        }
        from += 1;
    }
}

fn issue_body(environment: &str, lines: &[&str]) -> String {
    let mut body = format!(
        "**What happened**\n<!-- What did you do, what did you expect, what happened instead? \
         Paste or drop screenshots here, and better-kumo-log.txt (just shown in the file manager) \
         for the full log. -->\n\n**Environment**\n{environment}\n"
    );
    if !lines.is_empty() {
        body.push_str(&format!(
            "\n<details><summary>Computer log (last {} lines, names and ids removed)</summary>\n\n\
             ```text\n{}\n```\n</details>\n",
            lines.len(),
            lines.join("\n")
        ));
    }
    body
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn reveal(path: &Path) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(drop)
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(drop)
    } else {
        open::that(path.parent().unwrap_or(path))
    };
    if let Err(e) = result {
        eprintln!("report: cannot show {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yesterdays_or_oversized_log_moves_aside_and_old_ones_go() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("better-kumo.log");
        let config = LogsConfig {
            keep_days: 2,
            max_mb: 4,
        };
        let now = SystemTime::now();
        let saved = dir.path().join("better-kumo.backup.log");
        std::fs::write(&saved, vec![b'x'; 5_000_000]).unwrap();
        std::fs::write(&log, "one\ntwo\nthree\n").unwrap();
        assert!(!rotate(&log, &config, now));
        assert!(rotate(&log, &config, now + DAY));
        assert!(!log.exists());
        assert_eq!(rotated(&log).len(), 1);
        std::fs::write(&log, "four\nfive\n").unwrap();
        assert_eq!(tail(&log, 3), "three\nfour\nfive");
        assert_eq!(tail(&log, 1), "five");
        assert_eq!(log_bytes(&log), 14 + 10);

        std::fs::write(&log, vec![b'x'; 1_100_000]).unwrap();
        assert!(rotate(&log, &config, now + DAY));
        assert_eq!(rotated(&log).len(), 2);

        let (newest, _) = rotated(&log)
            .into_iter()
            .find(|(p, _)| p.to_string_lossy().ends_with("-2.log"))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&newest)
            .unwrap()
            .set_modified(now + DAY * 2)
            .unwrap();
        assert!(!rotate(&log, &config, now + DAY * 3));
        assert_eq!(rotated(&log).len(), 1);

        let tight = LogsConfig {
            keep_days: 30,
            max_mb: 1,
        };
        std::fs::write(&log, "new\n").unwrap();
        assert!(rotate(&log, &tight, now + DAY * 3));
        let left = rotated(&log);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].1.len(), 4);
        assert_eq!(tail(&dir.path().join("missing.log"), 5), "");
        assert!(saved.exists());

        let least = LogsConfig {
            keep_days: 30,
            max_mb: 1,
        };
        std::fs::write(&log, vec![b'y'; 300_000]).unwrap();
        assert!(rotate(&log, &least, now + DAY * 3));
        assert!(rotated(&log).iter().any(|(_, m)| m.len() == 300_000));

        let export = dir.path().join(EXPORT);
        std::fs::write(&export, "report").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&export)
            .unwrap()
            .set_modified(now + DAY * 3)
            .unwrap();
        assert!(!rotate(&log, &least, now + DAY * 3));
        assert!(export.exists());
        rotate(&log, &least, now + DAY * 5);
        assert!(!export.exists());
    }

    #[test]
    fn tail_skips_a_line_cut_by_the_read_window() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("better-kumo.log");
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&log, text).unwrap();
        let got = tail(&log, 3);
        assert_eq!(got, "line 1997\nline 1998\nline 1999");
    }

    #[test]
    fn redacts_people_ids_and_the_office() {
        let redactor = Redactor::new(
            vec![
                ("gobabygo".into(), "<space>".into()),
                ("kumospace_ihor".into(), "<topic>".into()),
                ("/srv/notes/Meetings".into(), MEETINGS.into()),
            ],
            [
                "Roman Honcharov".to_string(),
                "Pinto".into(),
                "Al".into(),
                "Sándor Kovács".into(),
                "Amy (QA)".into(),
                "王伟".into(),
            ],
            Some("Ihor Kravtsov".into()),
        );
        let log = "kumo-relay: space gobabygo as e4iPZ8cIrKb4KKV2kkF8cXiH0wp2, topic kumospace_ihor\n\
                   desktop: Nudge (Roman Honcharov nudged you)\n\
                   desktop: FloorEntry (Pinto joined) then Sándor Kovács, Roman Honcharov again; Al, Alice, Pintos\n\
                   Amy (QA) said hi to 王伟.\n\
                   meeting with Ihor Kravtsov in 01-86282603-2ff7-4b9b-a406-e3e474b61ab4.wav\n\
                   desktop: FloorEntry (Kseniia Muzyka joined the Floor)\n\
                   desktop: Knock (Amy is knocking)\n\
                   desktop: NearbyChat (Amy · Nearby Chat)\n\
             desktop: connected to ws://127.0.0.1:9222/devtools/page/57E68A6FEAD0EE919F466A8D9F5D50BF\n\
                   saved /Users/someone/Documents/x.md and C:\\Users\\Ann\\log.txt\n\
                   mail a.b@example.com from 192.168.1.20, fetch https://x.io/a?token=secret ok\n\
                   meetings: cannot compress /srv/notes/Meetings/2026-11-27 Board room: disk full\n\
                   meetings: deleted the audio of /Users/ann/Documents/Better Kumo/Meetings/Old room\n\
                   ntfy: ntfy 400 Bad Request: {\"message\":\"Amy: lunch?\"}";
        let got = redactor.apply(log);
        assert_eq!(
            got,
            "kumo-relay: space <space> as <my-id>, topic <topic>\n\
             desktop: Nudge (<title>)\n\
             desktop: FloorEntry (<person 2> joined) then <person 3>, <person 1> again; <person 4>, Alice, Pintos\n\
             <person 5> said hi to <person 6>.\n\
             meeting with <me> in 01-<uuid>.wav\n\
             desktop: FloorEntry (<title>)\n\
             desktop: Knock (<title>)\n\
             desktop: NearbyChat (<title>)\n\
             desktop: connected to ws://127.0.0.1:9222/devtools/page/<id>\n\
             saved /Users/<user>/Documents/x.md and C:\\Users\\<user>\\log.txt\n\
             mail <email> from <ip>, fetch https://x.io/a?… ok\n\
             meetings: cannot compress <meetings>/<meeting>: disk full\n\
             meetings: deleted the audio of <meetings>/<meeting>\n\
             ntfy: ntfy 400 Bad Request: <reply>"
        );
    }

    #[test]
    fn the_startup_line_loses_its_space_without_any_config() {
        let got = Redactor::new(Vec::new(), Vec::new(), None)
            .apply("kumo-relay: space oldoffice as e4iPZ8, ntfy=true, apns=false, mirroring=true");
        assert_eq!(
            got,
            "kumo-relay: space <space> as <my-id>, ntfy=true, apns=false, mirroring=true"
        );
    }

    #[test]
    fn the_issue_link_fits_by_dropping_the_oldest_lines() {
        let log: String = (0..500).map(|i| format!("log line number {i}\n")).collect();
        let url = issue_url("- Computer: Better Kumo 1.0 on macOS", &log);
        assert!(url.len() <= MAX_URL, "{}", url.len());
        assert!(url.starts_with(ISSUES_NEW));
        assert!(url.contains("log%20line%20number%20499"));
        assert!(!url.contains("log%20line%20number%2010%0A"));
        let long_first = format!("{}\nrecent one\nrecent two", "x".repeat(10_000));
        let url = issue_url("env", &long_first);
        assert!(url.len() <= MAX_URL);
        assert!(url.contains("recent%20one%0Arecent%20two"));
        assert!(!url.contains("xxxx"));
        let short = issue_url("env", "");
        assert!(!short.contains("details"));
        assert!(short.contains("%2A%2AWhat%20happened%2A%2A"));
    }
}
