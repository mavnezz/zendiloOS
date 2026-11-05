//! Looks up accounts in `/etc/passwd` and `/etc/group`. The accounts the
//! agent deals with (its client, the users and groups zendiloos manages)
//! are all local, so the files are authoritative for them.

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

/// GID of `group`, if `group_file` lists it.
pub fn group_id(group_file: &str, group: &str) -> Option<u32> {
    group_file.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != group {
            return None;
        }
        let _password = fields.next()?;
        fields.next()?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_groups() {
        let group = "root:x:0:\nzendiloos-users:x:990:\nfamily:x:2001:alice,bob\n";
        assert_eq!(group_id(group, "family"), Some(2001));
        assert_eq!(group_id(group, "zendiloos-users"), Some(990));
        assert_eq!(group_id(group, "zendiloos"), None);
    }

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
