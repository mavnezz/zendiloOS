-- When the schedule of image updates was set or last ran, Unix seconds
-- (zendiloos-containers, docs/architecture.md, 4.9): it is due once its
-- latest time lies after that, as a backup plan is, so a minute missed is
-- made up once. NULL: never noted (set before this was kept); due from the
-- first look on.
ALTER TABLE docker_update_settings ADD COLUMN schedule_since INTEGER;
