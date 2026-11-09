-- Bounded context Sharing: access to a share is the share's as a whole
-- (docs/architecture.md, 4.4). The rules stay as they were (per account
-- or group: deny, read, read and write); Samba enforces them per share,
-- zendiloosd for the file manager, and NFS clients are named by host with
-- every request squashed to the group zendiloos-files. On disk each share
-- has one ACL, that group's; no file carries per-person entries any more,
-- so nothing walks the shares when rules, groups or accounts change.
--
-- What the walks owed is gone (0038, 0041). Whose identity an NFS client's
-- requests keep is no choice any more: every rule is all_squash now.
--
-- `permissions_migrated_at` is when a share's files were given the one ACL:
-- at creation for shares made from now on, by a walk once for those made
-- before, which zendiloosd runs share by share, as a job, until each is
-- done (NULL until then). A rollback to a snapshot older than that walks
-- the share again.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE shares DROP COLUMN access_owed;
ALTER TABLE shares DROP COLUMN access_given;
ALTER TABLE share_nfs_clients DROP COLUMN squash;
ALTER TABLE shares ADD COLUMN permissions_migrated_at INTEGER;
