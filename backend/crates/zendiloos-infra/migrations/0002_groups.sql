-- Groups of accounts, each also a Linux group (docs/architecture.md, 4.2).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE groups (
    -- AUTOINCREMENT: an ID is never reused (see accounts).
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Also the Linux group name.
    name        TEXT    NOT NULL UNIQUE CHECK (name <> '' AND name = lower(name)),
    description TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE group_members (
    group_id   INTEGER NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, account_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX group_members_account_id ON group_members (account_id);
