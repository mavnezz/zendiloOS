-- Shares on the volumes and who may use them (docs/architecture.md, 4.4).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE shares (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Also the dataset, directory and SMB name.
    name        TEXT    NOT NULL UNIQUE CHECK (name <> '' AND name = lower(name)),
    description TEXT,
    -- The pool it lives on and the pool's volume number.
    pool        TEXT    NOT NULL,
    volume      INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999),
    recycle_bin INTEGER NOT NULL CHECK (recycle_bin IN (0, 1)),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

-- One row per account or group with access. Rules go with deleted
-- accounts and groups.
CREATE TABLE share_access (
    share_id   INTEGER NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    account_id INTEGER REFERENCES accounts (id) ON DELETE CASCADE,
    group_id   INTEGER REFERENCES groups (id) ON DELETE CASCADE,
    access     TEXT    NOT NULL CHECK (access IN ('read', 'read_write')),
    position   INTEGER NOT NULL,
    CHECK ((account_id IS NULL) <> (group_id IS NULL)),
    UNIQUE (share_id, account_id),
    UNIQUE (share_id, group_id)
) STRICT;

CREATE INDEX share_access_share ON share_access (share_id);

CREATE TABLE share_nfs_clients (
    share_id INTEGER NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    -- An IP address or CIDR network, normalized.
    network  TEXT    NOT NULL,
    write    INTEGER NOT NULL CHECK (write IN (0, 1)),
    position INTEGER NOT NULL,
    PRIMARY KEY (share_id, network)
) STRICT, WITHOUT ROWID;
