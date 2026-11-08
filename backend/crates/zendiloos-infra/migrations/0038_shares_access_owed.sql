-- Bounded context Sharing: what a share's files still owe its rules
-- (docs/architecture.md, 4.4). Whatever the files have to follow (new
-- rules, the members of a group the share names, an account disabled,
-- enabled again or deleted, a snapshot rolled back) raises `access_owed`
-- before anything changes. A walk through the files that read the rules
-- after the latest raise and ended without errors sets `access_given` to
-- it. While `access_given` is below `access_owed`, zendiloosd walks the
-- share again: when it starts, after each walk and every few minutes.
--
-- Walks used to take away only who their own change removed, and one that
-- failed or stopped was not made up: every share is walked once, so the
-- files of each get exactly the rules recorded for it.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE shares ADD COLUMN access_owed INTEGER NOT NULL DEFAULT 0 CHECK (access_owed >= 0);
ALTER TABLE shares ADD COLUMN access_given INTEGER NOT NULL DEFAULT 0 CHECK (access_given >= 0);
UPDATE shares SET access_owed = 1;
