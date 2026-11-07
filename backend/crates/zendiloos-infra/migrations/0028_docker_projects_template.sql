-- Bounded context Containers: the app of a catalog an administrator's
-- project was made from in the App Center (docs/architecture.md, 4.9). The
-- App Center fills in the app's template once; from then on the project is
-- the administrator's like any other, and these only let the App Center
-- find it again. Both are set or neither; the catalog may be gone since.
--
-- Migrations are immutable once written; change the schema with a new file.

ALTER TABLE docker_projects ADD COLUMN template_source INTEGER;
ALTER TABLE docker_projects ADD COLUMN template_app TEXT;
