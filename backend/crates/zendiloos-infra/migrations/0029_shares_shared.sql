-- Bounded context Sharing: a folder directly on a volume (a share's
-- dataset) is offered on the network, over SMB and NFS, only while it is
-- shared; otherwise it is used on the NAS alone, in the file manager and by
-- apps, with the same permissions (docs/architecture.md, 4.4). The shares
-- there were all on the network.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE shares ADD COLUMN shared INTEGER NOT NULL DEFAULT 1 CHECK (shared IN (0, 1));
