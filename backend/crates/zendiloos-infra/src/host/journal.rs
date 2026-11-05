//! The systemd journal through `journalctl --output=json`, run unprivileged
//! (zendiloosd is in group `systemd-journal`). Requests choose among fixed
//! views; only the cursor and numbers from a request reach the command
//! line, as single, validated arguments.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use zendiloos_exec::{Limits, Program};
use zendiloos_kernel::PortError;
use zendiloos_system::application::ports::JournalReader;
use zendiloos_system::domain::{
    JournalCursor, JournalEntry, JournalPage, JournalQuery, LogPriority, LogSource,
};

const WHAT: &str = "system journal";

/// Where `journalctl` is looked for if no path is configured: Debian, then
/// other common layouts (for development machines).
const CANDIDATES: [&str; 3] =
    ["/usr/bin/journalctl", "/bin/journalctl", "/run/current-system/sw/bin/journalctl"];

const FIELDS: &str = "MESSAGE,PRIORITY,_SYSTEMD_UNIT,SYSLOG_IDENTIFIER,_PID";

pub struct JournalctlReader {
    program: Option<Program>,
}

impl JournalctlReader {
    /// `path` if given, otherwise the first of the usual places that
    /// exists. Without one, reading fails with a clear error.
    pub fn new(path: Option<&Path>) -> Self {
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| CANDIDATES.iter().map(PathBuf::from).find(|candidate| candidate.exists()));
        Self { program: path.and_then(|path| Program::new(path).ok()) }
    }

    pub fn path(&self) -> Option<&Path> {
        self.program.as_ref().map(Program::path)
    }
}

fn arguments(query: &JournalQuery) -> Vec<String> {
    let mut args = vec![
        "--output=json".to_owned(),
        "--no-pager".to_owned(),
        "--quiet".to_owned(),
        // Newest first; with a cursor, the entries before it.
        "--reverse".to_owned(),
        format!("--lines={}", query.limit),
        format!("--priority=0..{}", query.priority.number()),
        format!("--output-fields={FIELDS}"),
    ];
    let units: &[&str] = match query.source {
        LogSource::System | LogSource::Kernel => &[],
        LogSource::Zendiloos => &["zendiloosd.service", "zendiloos-agent.service"],
        LogSource::Storage => &["mdmonitor.service", "smartmontools.service"],
        LogSource::Sharing => &["smbd.service", "nmbd.service"],
    };
    args.extend(units.iter().map(|unit| format!("--unit={unit}")));
    if query.source == LogSource::Kernel {
        args.push("--dmesg".to_owned());
    }
    if let Some(cursor) = &query.before {
        args.push(format!("--after-cursor={}", cursor.as_str()));
    }
    args
}

/// One entry as `journalctl --output=json` prints it: every value is a
/// string, except binary messages, which are arrays of bytes.
#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "__CURSOR")]
    cursor: String,
    #[serde(rename = "__REALTIME_TIMESTAMP")]
    realtime: String,
    #[serde(rename = "PRIORITY")]
    priority: Option<String>,
    #[serde(rename = "_SYSTEMD_UNIT")]
    unit: Option<String>,
    #[serde(rename = "SYSLOG_IDENTIFIER")]
    identifier: Option<String>,
    #[serde(rename = "_PID")]
    pid: Option<String>,
    #[serde(rename = "MESSAGE")]
    message: Option<Message>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Message {
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, thiserror::Error)]
#[error("unexpected journalctl output: {0}")]
struct ParseError(String);

fn parse(output: &str, limit: u16) -> Result<JournalPage, ParseError> {
    let mut entries = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Entry = serde_json::from_str(line).map_err(|e| ParseError(e.to_string()))?;
        let message = match entry.message {
            Some(Message::Text(text)) => text,
            Some(Message::Bytes(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::new(),
        };
        entries.push(JournalEntry {
            at_micros: entry.realtime.parse().map_err(|_| ParseError("timestamp".into()))?,
            // Entries without a priority are logged at info by convention.
            priority: entry
                .priority
                .and_then(|p| p.parse().ok())
                .and_then(LogPriority::from_number)
                .unwrap_or(LogPriority::Info),
            unit: entry.unit,
            identifier: entry.identifier,
            pid: entry.pid.and_then(|pid| pid.parse().ok()),
            message,
            cursor: JournalCursor::parse(&entry.cursor).map_err(|e| ParseError(e.to_string()))?,
        });
    }
    let older =
        (entries.len() >= usize::from(limit)).then(|| entries.last().map(|e| e.cursor.clone())).flatten();
    Ok(JournalPage { entries, older })
}

#[async_trait]
impl JournalReader for JournalctlReader {
    async fn read(&self, query: &JournalQuery) -> Result<JournalPage, PortError> {
        let program = self.program.as_ref().ok_or_else(|| PortError::new(WHAT, "journalctl not found"))?;
        let limits = Limits { timeout: Duration::from_secs(15), max_stdout: 8 * 1024 * 1024 };
        let output = zendiloos_exec::run(program, arguments(query), limits)
            .await
            .and_then(|output| output.success(program))
            .map_err(|e| PortError::new(WHAT, e))?;
        let text = String::from_utf8_lossy(&output.stdout);
        parse(&text, query.limit).map_err(|e| PortError::new(WHAT, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `journalctl --output=json --output-fields=…`, with a
    /// binary message added.
    const OUTPUT: &str = r#"{"__REALTIME_TIMESTAMP":"1793880897756376","PRIORITY":"3","MESSAGE":"smbd failed","__CURSOR":"s=07;i=21886c;b=9a;m=458;t=65f8;x=18","_SYSTEMD_UNIT":"smbd.service","_PID":"1898","SYSLOG_IDENTIFIER":"smbd","_BOOT_ID":"9a14"}
{"__MONOTONIC_TIMESTAMP":"16279217953","PRIORITY":"6","__REALTIME_TIMESTAMP":"1793878506839373","MESSAGE":"perf: interrupt took too long","__CURSOR":"s=07;i=217f68;b=9a;m=3ca;t=65f8;x=c8","SYSLOG_IDENTIFIER":"kernel"}
{"__REALTIME_TIMESTAMP":"1793878506000000","MESSAGE":[104,105,255],"__CURSOR":"s=07;i=217f67;b=9a;m=3c9;t=65f8;x=c7"}
"#;

    fn query(source: LogSource) -> JournalQuery {
        JournalQuery { source, priority: LogPriority::Warning, limit: 3, before: None }
    }

    #[test]
    fn reads_entries_newest_first() {
        let page = parse(OUTPUT, 3).unwrap();
        assert_eq!(page.entries.len(), 3);
        let first = &page.entries[0];
        assert_eq!(first.at_micros, 1_793_880_897_756_376);
        assert_eq!(first.priority, LogPriority::Error);
        assert_eq!(first.unit.as_deref(), Some("smbd.service"));
        assert_eq!(first.pid, Some(1898));
        assert_eq!(page.entries[1].identifier.as_deref(), Some("kernel"));
        assert_eq!(page.entries[2].message, "hi\u{fffd}");
        assert_eq!(page.entries[2].priority, LogPriority::Info);
        assert_eq!(
            page.older.as_ref().map(JournalCursor::as_str),
            Some("s=07;i=217f67;b=9a;m=3c9;t=65f8;x=c7")
        );
        assert_eq!(parse(OUTPUT, 10).unwrap().older, None, "fewer than asked for: nothing older");
        assert!(parse("not json", 3).is_err());
    }

    #[test]
    fn builds_fixed_command_lines() {
        let mut q = query(LogSource::Sharing);
        q.before = Some(JournalCursor::parse("s=07;i=1").unwrap());
        assert_eq!(
            arguments(&q),
            [
                "--output=json",
                "--no-pager",
                "--quiet",
                "--reverse",
                "--lines=3",
                "--priority=0..4",
                "--output-fields=MESSAGE,PRIORITY,_SYSTEMD_UNIT,SYSLOG_IDENTIFIER,_PID",
                "--unit=smbd.service",
                "--unit=nmbd.service",
                "--after-cursor=s=07;i=1",
            ]
        );
        assert!(arguments(&query(LogSource::Kernel)).contains(&"--dmesg".to_owned()));
        assert!(!arguments(&query(LogSource::System)).iter().any(|a| a.starts_with("--unit")));
    }
}
