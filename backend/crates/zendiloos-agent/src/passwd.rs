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

/// GIDs of the groups that list `user` as a member.
pub fn memberships(group_file: &str, user: &str) -> Vec<u32> {
    group_file
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let [_, _, gid, members] = fields[..] else { return None };
            members.split(',').any(|member| member == user).then(|| gid.parse().ok()).flatten()
        })
        .collect()
}

/// Every account `passwd` lists: name, UID and primary GID.
pub fn accounts(passwd: &str) -> impl Iterator<Item = (&str, u32, u32)> {
    passwd.lines().filter_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        Some((name, fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
    })
}

/// Names `group_file` lists as members of `group`.
pub fn members<'a>(group_file: &'a str, group: &str) -> Vec<&'a str> {
    group_file
        .lines()
        .find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let [name, _, _, members] = fields[..] else { return None };
            (name == group).then(|| members.split(',').filter(|m| !m.is_empty()).collect())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_accounts() {
        let passwd = "root:x:0:0::/root:/bin/sh\nbroken\nanna:x:2001:990::/:/bin/false\nodd:x:uid:1::/:/\n";
        assert_eq!(accounts(passwd).collect::<Vec<_>>(), [("root", 0, 0), ("anna", 2001, 990)]);
    }

    #[test]
    fn finds_members() {
        let group = "root:x:0:\nfamily:x:2001:alice,bob\nempty:x:2002:\n";
        assert_eq!(members(group, "family"), ["alice", "bob"]);
        assert!(members(group, "empty").is_empty());
        assert!(members(group, "fami").is_empty());
    }

    #[test]
    fn finds_memberships() {
        let group = "root:x:0:\nfamily:x:2001:alice,bob\nfriends:x:2002:bobby\nwork:x:2003:bob\nbad:x:\n";
        assert_eq!(memberships(group, "bob"), [2001, 2003]);
        assert_eq!(memberships(group, "carol"), Vec::<u32>::new());
    }

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
