-- Apps installed from a catalog (docs/architecture.md, 4.9), and what their
-- accounts may do on the shares picked for them.
--
-- Migrations are immutable once written; change the schema with a new file.

-- The Sharing context writes these into the share's permissions beside the
-- people's rules; a share an app uses cannot be deleted.
CREATE TABLE share_app_access (
    share_id INTEGER NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    -- The app's Linux account, app-<id>.
    account  TEXT    NOT NULL CHECK (account LIKE 'app-%'),
    access   TEXT    NOT NULL CHECK (access IN ('read', 'read_write')),
    PRIMARY KEY (share_id, account)
) STRICT, WITHOUT ROWID;

-- The version installed is kept whole (manifest, compose file, digest), so
-- the app can be installed again or removed even if its catalog is gone.
-- The values hold settings, some of them passwords: like the rest of the
-- database, readable only by zendiloosd.
CREATE TABLE installed_apps (
    id           TEXT    PRIMARY KEY,
    source_id    INTEGER NOT NULL,
    manifest     TEXT    NOT NULL,
    compose      TEXT    NOT NULL,
    icon         TEXT    NOT NULL,
    digest       TEXT    NOT NULL,
    pool         TEXT    NOT NULL,
    volume       INTEGER NOT NULL CHECK (volume BETWEEN 1 AND 999),
    -- JSON objects: share id to share name, port id to port, setting id to value.
    shares       TEXT    NOT NULL,
    ports        TEXT    NOT NULL,
    settings     TEXT    NOT NULL,
    state        TEXT    NOT NULL CHECK (state IN ('installing', 'installed', 'failed', 'removing')),
    installed_at INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;
