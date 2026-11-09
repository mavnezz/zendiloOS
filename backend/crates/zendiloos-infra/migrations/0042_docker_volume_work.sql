-- Containers a backup or restore of a volume stopped (zendiloos-containers)
-- and nothing started since: zendiloosd starts each again once no backup or
-- restore holds it any more and the agent said the work on its volume
-- ended, also after a restart, every minute until it runs, and forgets it
-- only once it started. They were kept with each backup's run before, and
-- not at all for restores.
CREATE TABLE docker_owed_starts (
    container TEXT    NOT NULL,
    -- The volume whose backup or restore stopped it.
    volume    TEXT    NOT NULL,
    since     INTEGER NOT NULL,
    PRIMARY KEY (container, volume)
) STRICT;

INSERT OR IGNORE INTO docker_owed_starts (container, volume, since)
SELECT stopped.value, docker_backups.volume, docker_backups.started_at
FROM docker_backups, json_each(docker_backups.stopped) AS stopped
WHERE docker_backups.stopped <> '[]';

ALTER TABLE docker_backups DROP COLUMN stopped;
