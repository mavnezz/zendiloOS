//! The console login prompt's text: where the web interface is and, while
//! the initial setup is pending, the setup code. agetty reads it as root
//! from `/run/issue.d`; the agent writes it from its own template, so
//! nothing zendiloosd controls is ever read there (a link planted by a
//! compromised zendiloosd could otherwise show any file on the console).

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use zendiloos_agent_proto::{ConsoleSetupCode, Response};

use crate::operations::OperationError;

/// The text. `\n` and `\4` are agetty escapes for the host name and its IPv4
/// address, filled in (and refreshed) by agetty.
pub(crate) fn issue_text(setup_code: Option<&ConsoleSetupCode>) -> String {
    let mut text = String::from("zendiloOS on \\n: https://\\4/\n");
    if let Some(code) = setup_code {
        text.push_str(&format!("Setup code for the first administrator: {}\n", code.as_str()));
    }
    text.push('\n');
    text
}

/// Replaces `path` atomically and privately (the setup code is a secret):
/// agetty never reads half a file.
pub(crate) fn show(path: &Path, setup_code: Option<&ConsoleSetupCode>) -> Result<Response, OperationError> {
    let temporary = path.with_extension("new");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| OperationError::Write(temporary.clone(), e))?;
    file.write_all(issue_text(setup_code).as_bytes())
        .map_err(|e| OperationError::Write(temporary.clone(), e))?;
    file.sync_all().map_err(|e| OperationError::Write(temporary.clone(), e))?;
    fs::rename(&temporary, path).map_err(|e| OperationError::Write(path.to_owned(), e))?;
    Ok(Response::Done)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn shows_the_address_and_a_pending_setup_code_privately() {
        let code = ConsoleSetupCode::parse("ABCD-EFGH-JKLM").unwrap();
        assert_eq!(
            issue_text(Some(&code)),
            "zendiloOS on \\n: https://\\4/\nSetup code for the first administrator: ABCD-EFGH-JKLM\n\n"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zendiloos.issue");
        // An older package linked it elsewhere; the link is replaced, not followed.
        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, "untouched").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();

        show(&path, Some(&code)).unwrap();
        show(&path, None).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "zendiloOS on \\n: https://\\4/\n\n");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "untouched");
    }
}
