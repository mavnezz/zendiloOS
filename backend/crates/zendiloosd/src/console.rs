//! What the console login prompt shows: where the web interface is and,
//! while the initial setup is pending, the setup code.
//!
//! zendiloosd writes an agetty issue file; the package links
//! `/run/issue.d/zendiloos.issue` to it. agetty runs as root and is the only
//! other reader; the file stays private to the service account.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The issue text. `\n` and `\4` are agetty escapes for the host name and
/// its IPv4 address, filled in (and refreshed) by agetty.
pub fn issue_text(setup_code: Option<&str>) -> String {
    let mut text = String::from("zendiloOS on \\n: https://\\4/\n");
    if let Some(code) = setup_code {
        text.push_str(&format!("Setup code for the first administrator: {code}\n"));
    }
    text.push('\n');
    text
}

/// Replaces `path` atomically, so agetty never reads half a file.
pub fn write_issue(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_address_and_a_pending_setup_code() {
        assert_eq!(
            issue_text(Some("ABCD-EFGH-JKLM")),
            "zendiloOS on \\n: https://\\4/\nSetup code for the first administrator: ABCD-EFGH-JKLM\n\n"
        );
        assert_eq!(issue_text(None), "zendiloOS on \\n: https://\\4/\n\n");
    }

    #[test]
    fn writes_a_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.issue");

        write_issue(&path, "first").unwrap();
        write_issue(&path, "second").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
