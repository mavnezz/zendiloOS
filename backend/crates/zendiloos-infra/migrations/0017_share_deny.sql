-- A rule can deny a share to an account or group, winning over every
-- grant (docs/architecture.md, 4.4). SQLite cannot change a CHECK
-- constraint, so the table is made again with the rows it had.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE share_access_new (
    share_id   INTEGER NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    account_id INTEGER REFERENCES accounts (id) ON DELETE CASCADE,
    group_id   INTEGER REFERENCES groups (id) ON DELETE CASCADE,
    access     TEXT    NOT NULL CHECK (access IN ('deny', 'read', 'read_write')),
    position   INTEGER NOT NULL,
    CHECK ((account_id IS NULL) <> (group_id IS NULL)),
    UNIQUE (share_id, account_id),
    UNIQUE (share_id, group_id)
) STRICT;

INSERT INTO share_access_new (share_id, account_id, group_id, access, position)
    SELECT share_id, account_id, group_id, access, position FROM share_access;

DROP TABLE share_access;
ALTER TABLE share_access_new RENAME TO share_access;
CREATE INDEX share_access_share ON share_access (share_id);
