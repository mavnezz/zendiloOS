-- Bounded context Containers: what an administrator confirmed for their
-- project (docs/architecture.md, 4.9). `compose` is the file as they saved
-- it, which the editor shows; `config` what Compose made of it then,
-- normalized as JSON, which is what runs: never the file in the share,
-- which whoever may write the share could change. Projects made before
-- have neither until their file is saved once.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE docker_projects ADD COLUMN compose TEXT;
ALTER TABLE docker_projects ADD COLUMN config TEXT;
