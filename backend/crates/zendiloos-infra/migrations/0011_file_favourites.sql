-- Folders each account keeps at hand in the file manager, in its order
-- (docs/architecture.md, 4.4). They go with the account or the share.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE file_favourites (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    share_id   INTEGER NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    path       TEXT    NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (account_id, share_id, path)
) STRICT;
