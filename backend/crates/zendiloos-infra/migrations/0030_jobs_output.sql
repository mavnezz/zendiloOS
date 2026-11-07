-- Jobs keep what a program they run wrote, for a person to follow: Docker
-- Compose deploying a project, as it pulls and makes containers. The last
-- lines only (zendiloos_jobs::OUTPUT_LINES), cleaned of terminal escapes
-- and control characters, one per line; empty for most jobs.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE jobs ADD COLUMN output TEXT NOT NULL DEFAULT '';
