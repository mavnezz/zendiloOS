-- A plan whose volume was gone when a run was due (zendiloos-containers):
-- it runs no more, and fails no more, until an administrator resumes it or
-- sets it anew once a volume of the name is there again. Its recorded
-- backups stay until they are removed.
ALTER TABLE docker_backup_plans ADD COLUMN paused INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1));

-- A backup found again in a folder and recorded (for a database that lost
-- it): of no plan, so retention never removes it.
ALTER TABLE docker_backups ADD COLUMN imported INTEGER NOT NULL DEFAULT 0 CHECK (imported IN (0, 1));
