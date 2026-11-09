-- Bounded context Sharing: every share's files owe it its rules once more
-- (docs/architecture.md, 4.4).
--
-- Walks used to take a folder whose named entries the taking pass had all
-- removed (a share given to another group, an account disabled, someone
-- denied: their entries went or became ---) for a private one, as it was
-- left at mode 0700, and gave it and everything below it nothing. Walks
-- now tell the private places by where they are (the recycle bins), so
-- the next walk of each share gives such folders their rules again.
--
-- Migrations are immutable once written; change the schema with a new file.

UPDATE shares SET access_owed = access_owed + 1;
