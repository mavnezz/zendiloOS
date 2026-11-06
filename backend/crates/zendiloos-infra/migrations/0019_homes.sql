-- Personal folders: where they are (one pool at most; no row: off) and
-- which accounts have one, with how much each may store there. Folders
-- stay on disk when an account loses its row; the row goes with the
-- account.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE homes_location (
    id     INTEGER PRIMARY KEY CHECK (id = 1),
    pool   TEXT    NOT NULL,
    volume INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999)
) STRICT;

CREATE TABLE homes (
    account_id  INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    quota_bytes INTEGER CHECK (quota_bytes IS NULL OR quota_bytes > 0)
) STRICT;
