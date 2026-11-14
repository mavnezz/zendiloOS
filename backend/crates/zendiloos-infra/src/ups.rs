//! The UPS's state, read from a NUT server over its own protocol: one
//! connection, `LIST VAR <ups>`, the variables it answers with. The server
//! is the NAS's own (127.0.0.1) for a UPS on the NAS, another device's for
//! one that the NAS only follows; reading variables needs no account.
//!
//! zendiloosd asks itself rather than through the agent, whose unit has no
//! network.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use zendiloos_ups::application::{Target, UpsLink};

/// How long a server may take to connect and answer.
const PATIENCE: Duration = Duration::from_secs(4);
/// More than any UPS has to say; what comes after is not read.
const MOST: u64 = 256 * 1024;

/// [`UpsLink`] over TCP.
#[derive(Debug, Clone, Copy, Default)]
pub struct NutLink;

/// The value NUT put between quotes, with its escapes (`\"`, `\\`) undone.
fn unquoted(text: &str) -> Option<String> {
    let inner = text.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut value = String::with_capacity(inner.len());
    let mut escaped = false;
    for c in inner.chars() {
        if escaped || c != '\\' {
            value.push(c);
            escaped = false;
        } else {
            escaped = true;
        }
    }
    Some(value)
}

/// One line of the answer: `VAR <ups> <name> "<value>"`.
fn variable<'a>(line: &'a str, ups: &str) -> Option<(&'a str, String)> {
    let rest = line.strip_prefix("VAR ")?.strip_prefix(ups)?.strip_prefix(' ')?;
    let (name, value) = rest.split_once(' ')?;
    Some((name, unquoted(value)?))
}

async fn ask(target: &Target) -> Result<BTreeMap<String, String>, String> {
    let stream = TcpStream::connect((target.host.as_str(), target.port)).await.map_err(|e| e.to_string())?;
    let (reader, mut writer) = stream.into_split();
    writer.write_all(format!("LIST VAR {}\n", target.ups).as_bytes()).await.map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(reader.take(MOST)).lines();
    let mut variables = BTreeMap::new();
    let end = format!("END LIST VAR {}", target.ups);
    loop {
        let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? else {
            return Err("the server closed the connection before the end of its answer".into());
        };
        if line == end {
            break;
        }
        if let Some(error) = line.strip_prefix("ERR ") {
            // UNKNOWN-UPS, DRIVER-NOT-CONNECTED, DATA-STALE …
            return Err(format!("the server says {error}"));
        }
        if let Some((name, value)) = variable(&line, &target.ups) {
            variables.insert(name.to_owned(), value);
        }
    }
    let _ = writer.write_all(b"LOGOUT\n").await;
    Ok(variables)
}

#[async_trait]
impl UpsLink for NutLink {
    async fn read(&self, target: &Target) -> Result<BTreeMap<String, String>, String> {
        match tokio::time::timeout(PATIENCE, ask(target)).await {
            Ok(answer) => answer,
            Err(_) => Err("the server did not answer in time".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    /// A server that answers the first line it is sent with `answer`, and
    /// tells what that line was.
    async fn server(answer: &'static str) -> (Target, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let line = lines.next_line().await.unwrap().unwrap_or_default();
            writer.write_all(answer.as_bytes()).await.unwrap();
            line
        });
        (Target { host: "127.0.0.1".into(), port, ups: "ups".into() }, asked)
    }

    #[tokio::test]
    async fn a_server_s_list_becomes_the_variables() {
        let (target, asked) = server(
            "BEGIN LIST VAR ups\n\
             VAR ups battery.charge \"87\"\n\
             VAR ups ups.status \"OL CHRG\"\n\
             VAR ups ups.model \"5E 650i \\\"Eco\\\" \\\\ USB\"\n\
             VAR other ups.status \"OB\"\n\
             END LIST VAR ups\n",
        )
        .await;
        let variables = NutLink.read(&target).await.unwrap();
        assert_eq!(asked.await.unwrap(), "LIST VAR ups");
        assert_eq!(variables.get("ups.status").map(String::as_str), Some("OL CHRG"));
        assert_eq!(variables.get("battery.charge").map(String::as_str), Some("87"));
        assert_eq!(variables.get("ups.model").map(String::as_str), Some("5E 650i \"Eco\" \\ USB"));
        assert_eq!(variables.len(), 3, "another UPS's line is not this one's");
    }

    #[tokio::test]
    async fn what_the_server_refuses_or_cuts_short_is_no_answer() {
        let (target, _) = server("ERR UNKNOWN-UPS\n").await;
        assert_eq!(NutLink.read(&target).await, Err("the server says UNKNOWN-UPS".into()));
        let (target, _) = server("BEGIN LIST VAR ups\nVAR ups ups.status \"OL\"\n").await;
        assert!(NutLink.read(&target).await.unwrap_err().contains("closed the connection"));
        // Nobody listens there.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(NutLink.read(&Target { host: "127.0.0.1".into(), port, ups: "ups".into() }).await.is_err());
    }

    #[test]
    fn a_value_is_what_stands_between_the_quotes() {
        assert_eq!(unquoted("\"OL CHRG\"").as_deref(), Some("OL CHRG"));
        assert_eq!(unquoted("\"\"").as_deref(), Some(""));
        assert_eq!(unquoted("\"a \\\"b\\\" c\"").as_deref(), Some("a \"b\" c"));
        assert_eq!(unquoted("no quotes"), None);
        assert_eq!(variable("VAR ups battery.charge \"87\"", "ups"), Some(("battery.charge", "87".into())));
        assert_eq!(variable("VAR ups2 battery.charge \"87\"", "ups"), None);
        assert_eq!(variable("BEGIN LIST VAR ups", "ups"), None);
    }
}
