-- Bounded context Sharing: each NFS rule of a folder on a volume chooses
-- whose identity the client's requests keep (`none`: no_root_squash,
-- `root`: root_squash, `all`: all_squash), whether writes are answered
-- before they are on disk (async) and whether ports above 1024 are taken
-- (insecure). The rules there were all exported with root squashed, sync
-- and from privileged ports only, which the defaults keep. `network` now
-- also holds a host name or a pattern such as *.example.com
-- (docs/architecture.md, 4.4).
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE share_nfs_clients
    ADD COLUMN squash TEXT NOT NULL DEFAULT 'root' CHECK (squash IN ('none', 'root', 'all'));
ALTER TABLE share_nfs_clients ADD COLUMN async INTEGER NOT NULL DEFAULT 0 CHECK (async IN (0, 1));
ALTER TABLE share_nfs_clients ADD COLUMN insecure INTEGER NOT NULL DEFAULT 0 CHECK (insecure IN (0, 1));
