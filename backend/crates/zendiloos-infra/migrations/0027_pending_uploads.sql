-- Bounded context Sharing: uploads under way, each with its hidden file
-- (`.zendiloos-upload-…`). An upload that zendiloosd could not finish nor
-- remove, because it stopped in the middle, leaves that file behind; when
-- it starts again, it removes what is listed here from before
-- (docs/architecture.md, 4.4). `share` is empty for a personal folder.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE pending_uploads (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user       TEXT    NOT NULL CHECK (user <> ''),
    volume     INTEGER NOT NULL,
    share      TEXT    NOT NULL,
    path       TEXT    NOT NULL CHECK (path <> ''),
    started_at INTEGER NOT NULL
) STRICT;
