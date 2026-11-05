//! Looks up the client account in `/etc/passwd`. The service account is a
//! local system account, so the file is authoritative for it.

/// UID and primary GID of `user`, if `passwd` lists it.
pub fn lookup(passwd: &str, user: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != user {
            return None;
        }
        let _password = fields.next()?;
        let uid = fields.next()?.parse().ok()?;
        let gid = fields.next()?.parse().ok()?;
        Some((uid, gid))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\n\
                          zendiloos:x:996:993:zendiloos:/var/lib/zendiloos:/usr/sbin/nologin\n\
                          broken:x:abc:1::/:/bin/false\n";

    #[test]
    fn finds_uid_and_gid() {
        assert_eq!(lookup(PASSWD, "zendiloos"), Some((996, 993)));
        assert_eq!(lookup(PASSWD, "root"), Some((0, 0)));
    }

    #[test]
    fn misses_unknown_and_malformed_entries() {
        assert_eq!(lookup(PASSWD, "zendilo"), None, "no prefix matches");
        assert_eq!(lookup(PASSWD, "broken"), None);
        assert_eq!(lookup(PASSWD, ""), None);
    }
}
