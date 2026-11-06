-- Bounded context Containers: the Compose projects administrators made
-- here, with their folder in a share (docs/architecture.md, 4.9). Docker
-- knows their containers; this keeps where their Compose file is.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE docker_projects (
    name       TEXT    PRIMARY KEY CHECK (name <> '' AND name = lower(name)),
    share_id   INTEGER NOT NULL,
    -- In the share: `docker/teslamate`.
    folder     TEXT    NOT NULL CHECK (folder <> ''),
    -- On the machine: `/volume1/docker/docker/teslamate` and its file.
    directory  TEXT    NOT NULL,
    file       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    created_by TEXT    NOT NULL
) STRICT;
