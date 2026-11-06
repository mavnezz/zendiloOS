-- More of Samba's settings, the discovery services and the Time Machine
-- share (control panel, File services). Defaults as before: Samba's own,
-- shares nobody may open hidden, wsdd2 and Bonjour on.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE file_services ADD COLUMN smb_encryption TEXT NOT NULL DEFAULT 'default'
    CHECK (smb_encryption IN ('default', 'required', 'off'));
ALTER TABLE file_services ADD COLUMN smb_signing TEXT NOT NULL DEFAULT 'default'
    CHECK (smb_signing IN ('default', 'mandatory', 'disabled'));
ALTER TABLE file_services ADD COLUMN hide_unreadable INTEGER NOT NULL DEFAULT 1 CHECK (hide_unreadable IN (0, 1));
ALTER TABLE file_services ADD COLUMN multichannel INTEGER NOT NULL DEFAULT 1 CHECK (multichannel IN (0, 1));
ALTER TABLE file_services ADD COLUMN oplocks INTEGER NOT NULL DEFAULT 1 CHECK (oplocks IN (0, 1));
ALTER TABLE file_services ADD COLUMN leases INTEGER NOT NULL DEFAULT 1 CHECK (leases IN (0, 1));
ALTER TABLE file_services ADD COLUMN durable_handles INTEGER NOT NULL DEFAULT 0 CHECK (durable_handles IN (0, 1));
ALTER TABLE file_services ADD COLUMN mac_compat INTEGER NOT NULL DEFAULT 1 CHECK (mac_compat IN (0, 1));
ALTER TABLE file_services ADD COLUMN follow_symlinks INTEGER NOT NULL DEFAULT 1 CHECK (follow_symlinks IN (0, 1));
ALTER TABLE file_services ADD COLUMN veto_files TEXT;
ALTER TABLE file_services ADD COLUMN delete_veto_files INTEGER NOT NULL DEFAULT 0 CHECK (delete_veto_files IN (0, 1));
ALTER TABLE file_services ADD COLUMN wsdd INTEGER NOT NULL DEFAULT 1 CHECK (wsdd IN (0, 1));
ALTER TABLE file_services ADD COLUMN bonjour INTEGER NOT NULL DEFAULT 1 CHECK (bonjour IN (0, 1));
-- No foreign key: a share deleted since simply is no Time Machine share.
ALTER TABLE file_services ADD COLUMN time_machine_share INTEGER;
ALTER TABLE file_services ADD COLUMN time_machine_max_bytes INTEGER
    CHECK (time_machine_max_bytes IS NULL OR time_machine_max_bytes > 0);
