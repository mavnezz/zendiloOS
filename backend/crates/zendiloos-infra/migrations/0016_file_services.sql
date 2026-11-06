-- Which file services run, and Samba's settings for all shares (control
-- panel, File services). One row at most; without it, the defaults
-- the package's smb.conf has: SMB and NFS on, WORKGROUP, SMB 2.1 at least.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE file_services (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    smb INTEGER NOT NULL CHECK (smb IN (0, 1)),
    nfs INTEGER NOT NULL CHECK (nfs IN (0, 1)),
    workgroup TEXT NOT NULL CHECK (length(workgroup) BETWEEN 1 AND 15),
    smb_min_protocol TEXT NOT NULL CHECK (smb_min_protocol IN ('smb2', 'smb2_1', 'smb3'))
) STRICT;
