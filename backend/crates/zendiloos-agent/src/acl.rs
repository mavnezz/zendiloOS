//! POSIX ACLs as the kernel keeps them, in the extended attributes
//! `system.posix_acl_access` and `system.posix_acl_default`: read and
//! written directly, so a share's whole tree is changed without starting a
//! program for every file.
//!
//! The edits are those of `setfacl -x` and `-m`: entries of named users and
//! groups are removed or set, everything else stays, and the mask becomes
//! the union of the group class (the owning group and every named entry).
//! An ACL without named entries keeps no mask; the kernel then keeps it as
//! the file's mode alone.

use zendiloos_agent_proto::SharePermission;

pub(crate) const ACCESS: &str = "system.posix_acl_access";
pub(crate) const DEFAULT: &str = "system.posix_acl_default";

const VERSION: u32 = 2;
const USER_OBJ: u16 = 0x01;
const USER: u16 = 0x02;
const GROUP_OBJ: u16 = 0x04;
const GROUP: u16 = 0x08;
const MASK: u16 = 0x10;
const OTHER: u16 = 0x20;
const UNDEFINED_ID: u32 = u32::MAX;
const READ: u16 = 4;
const WRITE: u16 = 2;
const EXECUTE: u16 = 1;

/// A named entry's subject, by ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Who {
    User(u32),
    Group(u32),
}

impl Who {
    fn key(self) -> (u16, u32) {
        match self {
            Who::User(uid) => (USER, uid),
            Who::Group(gid) => (GROUP, gid),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    tag: u16,
    perm: u16,
    id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Acl {
    entries: Vec<Entry>,
}

impl Acl {
    /// The ACL a file without one has: its mode's three classes.
    pub(crate) fn from_mode(mode: u32) -> Self {
        let class = |shift: u32| ((mode >> shift) & 0o7) as u16;
        Self {
            entries: vec![
                Entry { tag: USER_OBJ, perm: class(6), id: UNDEFINED_ID },
                Entry { tag: GROUP_OBJ, perm: class(3), id: UNDEFINED_ID },
                Entry { tag: OTHER, perm: class(0), id: UNDEFINED_ID },
            ],
        }
    }

    /// The attribute's value, or `None` if it is not one the kernel writes.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        let (header, rest) = bytes.split_first_chunk::<4>()?;
        if u32::from_le_bytes(*header) != VERSION || rest.len() % 8 != 0 {
            return None;
        }
        let (chunks, _) = rest.as_chunks::<8>();
        let entries = chunks
            .iter()
            .map(|chunk| Entry {
                tag: u16::from_le_bytes([chunk[0], chunk[1]]),
                perm: u16::from_le_bytes([chunk[2], chunk[3]]),
                id: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            })
            .collect();
        let acl = Self { entries };
        acl.is_valid().then_some(acl)
    }

    /// Each base entry once, named entries once each, a mask if there are
    /// named ones: what the kernel takes.
    fn is_valid(&self) -> bool {
        let count = |tag| self.entries.iter().filter(|e| e.tag == tag).count();
        let known =
            self.entries.iter().all(|e| [USER_OBJ, USER, GROUP_OBJ, GROUP, MASK, OTHER].contains(&e.tag));
        let mut named: Vec<(u16, u32)> =
            self.entries.iter().filter(|e| e.tag == USER || e.tag == GROUP).map(|e| (e.tag, e.id)).collect();
        let total = named.len();
        named.sort_unstable();
        named.dedup();
        known
            && count(USER_OBJ) == 1
            && count(GROUP_OBJ) == 1
            && count(OTHER) == 1
            && count(MASK) <= 1
            && named.len() == total
            && (total == 0 || count(MASK) == 1)
    }

    /// The attribute's value, entries in the order the kernel wants.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut entries = self.entries.clone();
        entries.sort_by_key(|e| (e.tag, e.id));
        let mut bytes = Vec::with_capacity(4 + 8 * entries.len());
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        for entry in entries {
            bytes.extend_from_slice(&entry.tag.to_le_bytes());
            bytes.extend_from_slice(&entry.perm.to_le_bytes());
            bytes.extend_from_slice(&entry.id.to_le_bytes());
        }
        bytes
    }

    /// A default ACL to start from: the base entries of this one, as
    /// setfacl makes it when a folder gets its first default entry.
    fn base(&self) -> Self {
        Self {
            entries: self
                .entries
                .iter()
                .filter(|e| [USER_OBJ, GROUP_OBJ, OTHER].contains(&e.tag))
                .copied()
                .collect(),
        }
    }

    pub(crate) fn remove(&mut self, who: Who) {
        let (tag, id) = who.key();
        self.entries.retain(|e| (e.tag, e.id) != (tag, id));
    }

    pub(crate) fn set(&mut self, who: Who, perm: u16) {
        let (tag, id) = who.key();
        match self.entries.iter_mut().find(|e| (e.tag, e.id) == (tag, id)) {
            Some(entry) => entry.perm = perm,
            None => self.entries.push(Entry { tag, perm, id }),
        }
    }

    /// The mask as setfacl computes it, or none without named entries.
    pub(crate) fn fix_mask(&mut self) {
        self.entries.retain(|e| e.tag != MASK);
        let named = self.entries.iter().any(|e| e.tag == USER || e.tag == GROUP);
        if named {
            let perm = self
                .entries
                .iter()
                .filter(|e| [GROUP_OBJ, USER, GROUP].contains(&e.tag))
                .fold(0, |all, e| all | e.perm);
            self.entries.push(Entry { tag: MASK, perm, id: UNDEFINED_ID });
        }
    }

    #[cfg(test)]
    pub(crate) fn named(&self, who: Who) -> Option<u16> {
        let (tag, id) = who.key();
        self.entries.iter().find(|e| (e.tag, e.id) == (tag, id)).map(|e| e.perm)
    }

    #[cfg(test)]
    fn mask(&self) -> Option<u16> {
        self.entries.iter().find(|e| e.tag == MASK).map(|e| e.perm)
    }
}

/// A share's rules for what is in it: who loses their entries, who gets
/// which. Folders get them as default entries too, for what is made in
/// them later.
#[derive(Debug, Clone, Default)]
pub(crate) struct Rules {
    pub(crate) remove: Vec<Who>,
    pub(crate) grant: Vec<(Who, SharePermission)>,
}

/// What a permission is on a folder, and in a folder's default ACL.
fn folder_perm(permission: SharePermission) -> u16 {
    match permission {
        SharePermission::Deny => 0,
        SharePermission::Read => READ | EXECUTE,
        SharePermission::ReadWrite => READ | WRITE | EXECUTE,
    }
}

/// On a file, execute only where somebody may execute it already
/// (setfacl's `X`): sharing a folder does not make its files programs.
fn file_perm(permission: SharePermission, executable: bool) -> u16 {
    let execute = if executable { EXECUTE } else { 0 };
    match permission {
        SharePermission::Deny => 0,
        SharePermission::Read => READ | execute,
        SharePermission::ReadWrite => READ | WRITE | execute,
    }
}

/// The access ACL `current` (the mode's if `None`) without `rules.remove`.
/// `None` if nothing changes.
pub(crate) fn without(current: Option<Acl>, mode: u32, rules: &Rules) -> Option<Acl> {
    let before = current.unwrap_or_else(|| Acl::from_mode(mode));
    let mut acl = before.clone();
    for who in &rules.remove {
        acl.remove(*who);
    }
    if acl == before {
        return None;
    }
    acl.fix_mask();
    Some(acl)
}

/// The access ACL `current` (the mode's if `None`) with `rules.grant`.
/// `None` if nothing changes.
pub(crate) fn with(current: Option<Acl>, mode: u32, folder: bool, rules: &Rules) -> Option<Acl> {
    let before = current.unwrap_or_else(|| Acl::from_mode(mode));
    let mut acl = before.clone();
    let executable = folder || mode & 0o111 != 0;
    for (who, permission) in &rules.grant {
        let perm = if folder { folder_perm(*permission) } else { file_perm(*permission, executable) };
        acl.set(*who, perm);
    }
    acl.fix_mask();
    (acl != before).then_some(acl)
}

/// A folder's default ACL `current` without `rules.remove`; `None` if
/// nothing changes (or it has none).
pub(crate) fn default_without(current: Option<Acl>, rules: &Rules) -> Option<Acl> {
    let before = current?;
    let mut acl = before.clone();
    for who in &rules.remove {
        acl.remove(*who);
    }
    if acl == before {
        return None;
    }
    acl.fix_mask();
    Some(acl)
}

/// A folder's default ACL `current` with `rules.grant`, made from `access`
/// (the folder's access ACL) if it has none. `None` if nothing changes.
pub(crate) fn default_with(current: Option<Acl>, access: &Acl, rules: &Rules) -> Option<Acl> {
    if rules.grant.is_empty() {
        return None;
    }
    let before = current.clone();
    let mut acl = current.unwrap_or_else(|| access.base());
    for (who, permission) in &rules.grant {
        acl.set(*who, folder_perm(*permission));
    }
    acl.fix_mask();
    (Some(&acl) != before.as_ref()).then_some(acl)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(remove: &[Who], grant: &[(Who, SharePermission)]) -> Rules {
        Rules { remove: remove.to_vec(), grant: grant.to_vec() }
    }

    #[test]
    fn reads_back_what_it_writes_in_the_kernels_order() {
        let mut acl = Acl::from_mode(0o750);
        acl.set(Who::Group(2000), 5);
        acl.set(Who::User(1001), 7);
        acl.set(Who::User(1000), 4);
        acl.fix_mask();
        let bytes = acl.to_bytes();
        assert_eq!(bytes.len(), 4 + 8 * 7);
        let tags: Vec<u16> =
            bytes[4..].as_chunks::<8>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(tags, [USER_OBJ, USER, USER, GROUP_OBJ, GROUP, MASK, OTHER]);
        let ids: Vec<u32> = bytes[4..]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes([c[4], c[5], c[6], c[7]]))
            .collect();
        assert_eq!(ids[1..3], [1000, 1001]);
        let back = Acl::parse(&bytes).unwrap();
        assert_eq!(back.named(Who::User(1001)), Some(7));
        assert_eq!(back.mask(), Some(7), "the union of the group class");
    }

    #[test]
    fn refuses_what_the_kernel_would_not_write() {
        assert!(Acl::parse(&[1, 0, 0, 0]).is_none(), "another version");
        assert!(Acl::parse(&[2, 0, 0, 0, 1]).is_none(), "a torn entry");
        let mut named_without_mask = Acl::from_mode(0o700);
        named_without_mask.set(Who::User(1000), 7);
        assert!(Acl::parse(&named_without_mask.to_bytes()).is_none());
        assert!(Acl::parse(&Acl::from_mode(0o640).to_bytes()).is_some());
    }

    #[test]
    fn removing_the_last_named_entry_leaves_no_mask() {
        let mut acl = Acl::from_mode(0o770);
        acl.set(Who::User(1000), 7);
        acl.fix_mask();
        let gone = without(Some(acl), 0o770, &rules(&[Who::User(1000)], &[])).unwrap();
        assert_eq!(gone, Acl::from_mode(0o770));
        assert!(without(None, 0o640, &rules(&[Who::User(1000)], &[])).is_none(), "nothing to remove");
    }

    #[test]
    fn files_get_execute_only_where_somebody_has_it() {
        let grant = rules(
            &[],
            &[(Who::User(1000), SharePermission::ReadWrite), (Who::Group(2000), SharePermission::Read)],
        );
        let plain = with(None, 0o640, false, &grant).unwrap();
        assert_eq!(plain.named(Who::User(1000)), Some(6));
        assert_eq!(plain.named(Who::Group(2000)), Some(4));
        let program = with(None, 0o750, false, &grant).unwrap();
        assert_eq!(program.named(Who::User(1000)), Some(7));
        let folder = with(None, 0o750, true, &grant).unwrap();
        assert_eq!(folder.named(Who::Group(2000)), Some(5));
        let denied =
            with(None, 0o750, true, &rules(&[], &[(Who::User(1000), SharePermission::Deny)])).unwrap();
        assert_eq!(denied.named(Who::User(1000)), Some(0));
        assert!(with(Some(folder.clone()), 0o770, true, &grant).is_none(), "already so");
    }

    #[test]
    fn a_first_default_entry_brings_the_base_entries_along() {
        let access = Acl::from_mode(0o750);
        let grant = rules(&[], &[(Who::User(1000), SharePermission::Read)]);
        let default = default_with(None, &access, &grant).unwrap();
        let mut expected = Acl::from_mode(0o750);
        expected.set(Who::User(1000), 5);
        expected.fix_mask();
        assert_eq!(default, expected);
        assert!(default_with(None, &access, &rules(&[Who::User(1)], &[])).is_none());
        assert!(default_without(None, &rules(&[Who::User(1000)], &[])).is_none(), "none to change");
        let gone = default_without(Some(default), &rules(&[Who::User(1000)], &[])).unwrap();
        assert_eq!(gone, Acl::from_mode(0o750));
    }
}
