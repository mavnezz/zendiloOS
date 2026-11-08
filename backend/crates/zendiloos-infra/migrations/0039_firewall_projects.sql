-- The firewall (zendiloos-firewall, docs/architecture.md, 4.12): the host
-- ports Docker's projects published when Docker last answered. After a
-- restart they stand for the projects until Docker answers again, so a rule
-- about a project holds from the start.
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE firewall_project_ports (
    project   TEXT    NOT NULL CHECK (project <> ''),
    transport TEXT    NOT NULL CHECK (transport IN ('tcp', 'udp')),
    port      INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    PRIMARY KEY (project, transport, port)
) STRICT;
