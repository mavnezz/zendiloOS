//! POSIX ACLs as the kernel keeps them, in the extended attributes
//! `system.posix_acl_access` and `system.posix_acl_default`: read and
//! written directly, so a share's whole tree is changed without starting a
//! program for every file.
//!
//! A share's access is the share's as a whole, enforced by Samba and
//! zendiloosd (docs/architecture.md, 4.4); on disk every share has one ACL:
//! the group `zendiloos-files` may read and write, and what is made in a
//! folder inherits that. The edits here are those of `setfacl -x` and `-m`:
//! named entries are removed or set, and the mask becomes the union of the
//! group class (the owning group and every named entry). An ACL without
//! named entries keeps no mask; the kernel then keeps it as the file's mode
//! alone.

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
const ALL: u16 = READ | WRITE | EXECUTE;

/// A named entry's subject, by ID. Only groups are given entries; the
/// tests look at users' too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Who {
    #[cfg(test)]
    User(u32),
    Group(u32),
}

impl Who {
    fn key(self) -> (u16, u32) {
        match self {
            #[cfg(test)]
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

    pub(crate) fn set(&mut self, who: Who, perm: u16) {
        let (tag, id) = who.key();
        match self.entries.iter_mut().find(|e| (e.tag, e.id) == (tag, id)) {
            Some(entry) => entry.perm = perm,
            None => self.entries.push(Entry { tag, perm, id }),
        }
    }

    /// The owner's, the owning group's and others' entries set to `mode`'s.
    fn set_base(&mut self, mode: u32) {
        let class = |shift: u32| ((mode >> shift) & 0o7) as u16;
        for entry in &mut self.entries {
            match entry.tag {
                USER_OBJ => entry.perm = class(6),
                GROUP_OBJ => entry.perm = class(3),
                OTHER => entry.perm = class(0),
                _ => {}
            }
        }
    }

    /// Without the named entries `managed` holds.
    fn strip(&mut self, managed: &Managed) {
        self.entries.retain(|e| !managed.holds(e));
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
    pub(crate) fn mask(&self) -> Option<u16> {
        self.entries.iter().find(|e| e.tag == MASK).map(|e| e.perm)
    }

    #[cfg(test)]
    pub(crate) fn has_named(&self) -> bool {
        self.entries.iter().any(|e| e.tag == USER || e.tag == GROUP)
    }
}

/// The IDs whose named entries share-level access replaces: the accounts
/// and groups zendiloOS manages (people and their groups, those gone since
/// among them) and `zendiloos-users`, every account's primary group. The
/// apps' accounts and groups (`app-<id>`, given shares before apps became
/// administrators' projects) keep theirs, so installed apps keep working;
/// so do IDs zendiloOS does not hand out (system accounts, the owner's own
/// user, the users containers run as).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Managed {
    pub(crate) ids: Option<std::ops::RangeInclusive<u32>>,
    /// GIDs outside `ids` whose group entries go too: `zendiloos-users`.
    pub(crate) groups: Vec<u32>,
    /// The apps' UIDs, whose user entries stay.
    pub(crate) app_users: Vec<u32>,
    /// The apps' GIDs, whose group entries stay.
    pub(crate) app_groups: Vec<u32>,
}

impl Managed {
    /// Whether `entry` is a named entry that goes.
    fn holds(&self, entry: &Entry) -> bool {
        let managed = |id: &u32| self.ids.as_ref().is_some_and(|ids| ids.contains(id));
        match entry.tag {
            USER => managed(&entry.id) && !self.app_users.contains(&entry.id),
            GROUP => {
                (managed(&entry.id) && !self.app_groups.contains(&entry.id))
                    || self.groups.contains(&entry.id)
            }
            _ => false,
        }
    }
}

/// What `zendiloos-files` gets on a file: read and write, and execute only
/// where somebody may execute it already (setfacl's `X`), so sharing a
/// folder does not make its files programs.
fn file_perm(mode: u32) -> u16 {
    if mode & 0o111 != 0 { ALL } else { READ | WRITE }
}

/// The access ACL `current` (the mode's if `None`) of an entry in a share:
/// the named entries `managed` holds gone, the group `files` with read and
/// write (and search, or execute where it is a program). Other named
/// entries and the base entries stay. `None` if nothing changes.
pub(crate) fn shared(
    current: Option<Acl>,
    mode: u32,
    folder: bool,
    files: u32,
    managed: &Managed,
) -> Option<Acl> {
    let before = current.unwrap_or_else(|| Acl::from_mode(mode));
    let mut acl = before.clone();
    acl.strip(managed);
    acl.set(Who::Group(files), if folder { ALL } else { file_perm(mode) });
    acl.fix_mask();
    (acl != before).then_some(acl)
}

/// A folder's default ACL `current` as in a share: as [`shared`] does, for
/// what is made in the folder later; made from `access` (the folder's
/// access ACL) if it has none. `None` if nothing changes.
pub(crate) fn shared_default(
    current: Option<Acl>,
    access: &Acl,
    files: u32,
    managed: &Managed,
) -> Option<Acl> {
    let before = current.clone();
    let mut acl = current.unwrap_or_else(|| access.base());
    acl.strip(managed);
    acl.set(Who::Group(files), ALL);
    acl.fix_mask();
    (Some(&acl) != before.as_ref()).then_some(acl)
}

/// The ACL, access or default, of a share's root (and of its `.recycle`):
/// owner and owning group read and write, others nothing (`mode`'s base
/// entries), the group `files` too; the named entries `managed` holds
/// gone, others kept (an app's, given the share before). A new share's
/// root has exactly `u::rwx, g::rwx, g:zendiloos-files:rwx, m::rwx,
/// o::---`.
pub(crate) fn root(current: Option<Acl>, mode: u32, files: u32, managed: &Managed) -> Acl {
    let mut acl = current.unwrap_or_else(|| Acl::from_mode(mode));
    acl.set_base(mode);
    acl.strip(managed);
    acl.set(Who::Group(files), ALL);
    acl.fix_mask();
    acl
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ACL of `mode` with these named entries and their mask.
    fn named(mode: u32, entries: &[(Who, u16)]) -> Acl {
        let mut acl = Acl::from_mode(mode);
        for (who, perm) in entries {
            acl.set(*who, *perm);
        }
        acl.fix_mask();
        acl
    }

    fn base(acl: &Acl, tag: u16) -> Option<u16> {
        acl.entries.iter().find(|e| e.tag == tag).map(|e| e.perm)
    }

    fn managed() -> Managed {
        Managed { ids: Some(2000..=59999), groups: vec![990], app_users: vec![2500], app_groups: vec![2501] }
    }

    #[test]
    fn named_entries_of_people_and_their_groups_go_apps_and_foreign_ones_stay() {
        let (anna, family, users, files) = (Who::User(2001), Who::Group(2100), Who::Group(990), 995);
        let (app, app_group, debian, user_990) =
            (Who::User(2500), Who::Group(2501), Who::User(1000), Who::User(990));
        let left = named(
            0o750,
            &[(anna, 7), (family, 5), (users, 7), (app, 7), (app_group, 5), (debian, 7), (user_990, 5)],
        );

        let now = shared(Some(left.clone()), 0o750, true, files, &managed()).unwrap();

        assert_eq!((now.named(anna), now.named(family), now.named(users)), (None, None, None));
        assert_eq!((now.named(app), now.named(app_group)), (Some(7), Some(5)), "an app keeps working");
        assert_eq!((now.named(debian), now.named(user_990)), (Some(7), Some(5)), "not ours to take");
        assert_eq!(now.named(Who::Group(files)), Some(7));
        assert_eq!(
            (base(&now, USER_OBJ), base(&now, GROUP_OBJ), base(&now, OTHER)),
            (Some(7), Some(5), Some(0))
        );
        assert_eq!(now.mask(), Some(7), "the union of what is left");
        assert!(shared(Some(now), 0o770, true, files, &managed()).is_none(), "so already");
    }

    #[test]
    fn files_get_execute_only_where_somebody_has_it() {
        let files = 995;
        let plain = shared(None, 0o640, false, files, &managed()).unwrap();
        assert_eq!(plain.named(Who::Group(files)), Some(6));
        assert_eq!(plain.mask(), Some(6));
        let program = shared(None, 0o750, false, files, &managed()).unwrap();
        assert_eq!(program.named(Who::Group(files)), Some(7));
        let private = shared(None, 0o600, false, files, &managed()).unwrap();
        assert_eq!(private.mask(), Some(6), "a private file is the share's too");
        assert_eq!(base(&private, OTHER), Some(0));
    }

    #[test]
    fn a_folders_default_acl_gives_what_comes_later_the_group() {
        let files = 995;
        let access = Acl::from_mode(0o700);
        let made = shared_default(None, &access, files, &managed()).unwrap();
        assert_eq!(made.named(Who::Group(files)), Some(7));
        assert_eq!(
            (base(&made, USER_OBJ), base(&made, GROUP_OBJ)),
            (Some(7), Some(0)),
            "from the folder's own"
        );
        let left = named(0o770, &[(Who::User(2001), 7), (Who::User(1000), 5)]);
        let now = shared_default(Some(left), &access, files, &managed()).unwrap();
        assert_eq!((now.named(Who::User(2001)), now.named(Who::User(1000))), (None, Some(5)));
        assert!(shared_default(Some(now), &access, files, &managed()).is_none(), "so already");
    }

    #[test]
    fn a_new_shares_root_has_exactly_the_one_acl() {
        let files = 995;
        let acl = root(None, 0o2770, files, &managed());
        let mut expected = Acl::from_mode(0o770);
        expected.set(Who::Group(files), 7);
        expected.fix_mask();
        assert_eq!(acl, expected);
        // An old root (owner root, 0770, an entry per person, one app's).
        let old = named(0o700, &[(Who::User(2001), 7), (Who::User(2500), 5)]);
        let acl = root(Some(old), 0o2770, files, &managed());
        assert_eq!((acl.named(Who::User(2001)), acl.named(Who::User(2500))), (None, Some(5)));
        assert_eq!((base(&acl, GROUP_OBJ), base(&acl, OTHER), acl.mask()), (Some(7), Some(0), Some(7)));
    }

    #[test]
    fn without_managed_ids_only_the_group_is_added() {
        let left = named(0o770, &[(Who::User(2001), 7)]);
        let now = shared(Some(left), 0o770, true, 995, &Managed::default()).unwrap();
        assert_eq!(now.named(Who::User(2001)), Some(7));
        assert_eq!(now.named(Who::Group(995)), Some(7));
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
        let mut acl = named(0o770, &[(Who::User(2001), 7)]);
        acl.strip(&managed());
        acl.fix_mask();
        assert_eq!(acl, Acl::from_mode(0o770));
        assert!(!acl.has_named());
    }
}
