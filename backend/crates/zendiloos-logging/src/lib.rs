//! Logging setup shared by zendiloosd and the agent.
//!
//! Logs go to stderr. Under systemd that is the journal, which adds its own
//! timestamps and reads the severity from a `<N>` prefix on each line, so
//! the log viewer can filter by priority.

use std::io::IsTerminal;

use anyhow::Context;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{self, FormatEvent, FormatFields, Writer};
use tracing_subscriber::registry::LookupSpan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Human-readable lines; adapted to the journal when writing to it.
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Installs the global subscriber. `RUST_LOG`, if set, overrides `filter`.
pub fn init(filter: &str, format: Format) -> anyhow::Result<()> {
    let filter = filter_from(std::env::var("RUST_LOG").ok(), filter)?;
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr);
    match format {
        Format::Text if logging_to_journal() => builder
            .with_ansi(false)
            .event_format(SyslogPrefix(journal_lines().without_time().with_level(false)))
            .try_init(),
        Format::Text => builder.with_ansi(std::io::stderr().is_terminal()).try_init(),
        Format::Json => builder.json().try_init(),
    }
    .map_err(|error| anyhow::anyhow!(error))
}

/// `RUST_LOG`'s directives (`environment`), if set and not empty, else
/// `default`.
fn filter_from(environment: Option<String>, default: &str) -> anyhow::Result<EnvFilter> {
    match environment {
        Some(directive) if !directive.is_empty() => {
            EnvFilter::try_new(&directive).context("invalid RUST_LOG")
        }
        _ => EnvFilter::try_new(default).context("invalid log filter"),
    }
}

/// Whether stderr is connected to the journal. systemd sets
/// `JOURNAL_STREAM` to the device and inode of that connection; the
/// variable alone is not enough, since child processes inherit it.
pub fn logging_to_journal() -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else { return false };
    let Ok(stderr) = std::fs::metadata("/proc/self/fd/2") else { return false };
    is_stream(&stream.to_string_lossy(), stderr.dev(), stderr.ino())
}

/// `value` is `JOURNAL_STREAM`'s `<device>:<inode>`.
fn is_stream(value: &str, device: u64, inode: u64) -> bool {
    value.split_once(':').is_some_and(|(d, i)| d.parse() == Ok(device) && i.parse() == Ok(inode))
}

/// The syslog priority of a level, as the journal understands it.
fn priority(level: Level) -> u8 {
    match level {
        Level::ERROR => 3,
        Level::WARN => 4,
        Level::INFO => 6,
        Level::DEBUG | Level::TRACE => 7,
    }
}

/// Prefixes each event with `<N>`, its syslog priority.
struct SyslogPrefix<F>(F);

impl<S, N, F> FormatEvent<S, N> for SyslogPrefix<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        write!(writer, "<{}>", priority(*event.metadata().level()))?;
        self.0.format_event(context, writer, event)
    }
}

fn journal_lines() -> format::Format {
    format::Format::default()
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Buffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn levels_have_the_journals_priorities() {
        let priorities: Vec<u8> = [Level::ERROR, Level::WARN, Level::INFO, Level::DEBUG, Level::TRACE]
            .into_iter()
            .map(priority)
            .collect();
        assert_eq!(priorities, [3, 4, 6, 7, 7]);
    }

    #[test]
    fn rust_log_wins_over_the_configured_filter() {
        assert_eq!(filter_from(Some("debug".into()), "info").unwrap().to_string(), "debug");
        assert_eq!(filter_from(Some(String::new()), "info").unwrap().to_string(), "info", "empty is not set");
        assert_eq!(filter_from(None, "warn").unwrap().to_string(), "warn");
        let bad = filter_from(Some("x=notalevel".into()), "info").unwrap_err();
        assert!(bad.to_string().contains("RUST_LOG"), "{bad}");
        let bad = filter_from(None, "x=notalevel").unwrap_err();
        assert!(bad.to_string().contains("log filter"), "{bad}");
    }

    #[test]
    fn installs_one_subscriber_and_refuses_a_broken_filter() {
        assert!(init("info,zendiloos=notalevel", Format::Text).is_err(), "not a level");
        // Once per process: the first takes, a second is refused.
        let first = init("info", Format::Json);
        let second = init("info", Format::Text);
        assert!(first.is_ok() || std::env::var("RUST_LOG").is_ok_and(|v| !v.is_empty()), "{first:?}");
        assert!(second.is_err(), "a subscriber is set already");
    }

    #[test]
    fn tells_whether_stderr_is_the_journal() {
        use std::os::unix::fs::MetadataExt;
        let expected = std::env::var("JOURNAL_STREAM").ok().is_some_and(|stream| {
            std::fs::metadata("/proc/self/fd/2")
                .is_ok_and(|stderr| is_stream(&stream, stderr.dev(), stderr.ino()))
        });
        assert_eq!(logging_to_journal(), expected);
    }

    #[test]
    fn recognizes_the_journal_stream() {
        assert!(is_stream("8:12345", 8, 12345));
        assert!(!is_stream("8:12345", 8, 12346), "inherited from another process");
        assert!(!is_stream("garbage", 8, 12345));
    }

    #[test]
    fn prefixes_lines_with_the_syslog_priority() {
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .event_format(SyslogPrefix(journal_lines().without_time().with_level(false)))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(op = "reboot", "operation failed");
            tracing::info!("started");
        });
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<_> = output.lines().collect();
        assert!(
            lines[0].starts_with("<4>") && lines[0].contains("operation failed op=\"reboot\""),
            "{output}"
        );
        assert!(lines[1].starts_with("<6>"), "{output}");
    }
}
