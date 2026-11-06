-- Catalogs apps come from (docs/architecture.md, 4.9): the community
-- catalog, built in, and those administrators add. The index text last read
-- is kept, so apps stay listed when a catalog cannot be reached.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE app_sources (
    id             INTEGER PRIMARY KEY,
    index_address  TEXT    NOT NULL UNIQUE,
    official       INTEGER NOT NULL DEFAULT 0 CHECK (official IN (0, 1)),
    added_at       INTEGER NOT NULL,
    refreshed_at   INTEGER,
    refresh_error  TEXT,
    skipped        INTEGER NOT NULL DEFAULT 0,
    index_text     TEXT
) STRICT;

INSERT INTO app_sources (index_address, official, added_at)
VALUES ('https://mavnezz.github.io/zendiloOS.community.applications/v1/index.json', 1, 0);

-- Icons by the app's digest: fetched once per version.
CREATE TABLE app_icons (
    source_id INTEGER NOT NULL REFERENCES app_sources (id) ON DELETE CASCADE,
    app_id    TEXT    NOT NULL,
    digest    TEXT    NOT NULL,
    png       BLOB    NOT NULL,
    PRIMARY KEY (source_id, app_id)
) STRICT;
