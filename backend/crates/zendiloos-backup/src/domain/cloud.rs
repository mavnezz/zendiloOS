//! Cloud accounts zendiloOS signs in to (Google Drive, OneDrive), and the
//! address a browser lands on after consenting, pasted back.

use zendiloos_kernel::Timestamp;

use super::task::Provider;

/// A cloud account as kept; its token (and its own client's secret) are
/// secrets kept beside it, never shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAccount {
    pub id: i64,
    /// What the administrator calls it.
    pub name: String,
    pub provider: Provider,
    /// The administrator's own OAuth client; none: rclone's public one.
    pub client_id: Option<String>,
    /// OneDrive's drive, as Microsoft Graph names it.
    pub drive_id: Option<String>,
    pub drive_type: Option<String>,
    pub created_at: Timestamp,
}

/// What the address a browser landed on carries back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    /// Consent given: the code to exchange, and the state it was asked with.
    Code { code: String, state: String },
    /// Refused or failed, in the provider's words.
    Error(String),
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' if value.get(at + 1..at + 3).is_some_and(|h| u8::from_str_radix(h, 16).is_ok()) => {
                out.push(u8::from_str_radix(&value[at + 1..at + 3], 16).unwrap_or(b'?'));
                at += 3;
                continue;
            }
            byte => out.push(byte),
        }
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The pasted address (`http://127.0.0.1:53682/?state=…&code=…`, or just
/// its query) read; `None` if it carries neither a code nor an error.
pub fn read_redirect(pasted: &str) -> Option<Redirect> {
    let pasted = pasted.trim();
    if pasted.len() > 8192 {
        return None;
    }
    let query = pasted.split_once('?').map_or(pasted, |(_, query)| query);
    let query = query.split('#').next().unwrap_or_default();
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(decode(value)),
            "state" => state = Some(decode(value)),
            "error_description" => error = Some(decode(value)),
            "error" if error.is_none() => error = Some(decode(value)),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Some(Redirect::Error(error));
    }
    match (code, state) {
        (Some(code), Some(state)) if !code.is_empty() && !state.is_empty() => {
            Some(Redirect::Code { code, state })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_a_browser_landed_on_is_read() {
        assert_eq!(
            read_redirect(" http://127.0.0.1:53682/?state=abc123&code=4%2F0Ab_x+y&scope=drive "),
            Some(Redirect::Code { code: "4/0Ab_x y".into(), state: "abc123".into() })
        );
        assert_eq!(
            read_redirect("state=s&code=c"),
            Some(Redirect::Code { code: "c".into(), state: "s".into() })
        );
        assert_eq!(
            read_redirect(
                "http://localhost:53682/?error=access_denied&error_description=The+user+said+no&state=s"
            ),
            Some(Redirect::Error("The user said no".into()))
        );
        assert_eq!(
            read_redirect("http://localhost:53682/?error=access_denied"),
            Some(Redirect::Error("access_denied".into()))
        );
        assert_eq!(read_redirect("http://127.0.0.1:53682/"), None);
        assert_eq!(read_redirect("code=&state=s"), None);
        assert_eq!(read_redirect(&"x".repeat(9000)), None);
        assert_eq!(
            read_redirect("code=%zz&state=s"),
            Some(Redirect::Code { code: "%zz".into(), state: "s".into() })
        );
    }
}
