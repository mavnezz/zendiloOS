-- Bounded context Certificates: accounts at an ACME server, the ways the
-- server is shown a name is ours (a DNS provider's API, with its values,
-- secrets among them, or plain HTTP), certificates, and the web
-- interface's ports and certificate. acme.sh keeps its own files on the
-- host; these say what it is asked for (docs/architecture.md, 4.11).
--
-- Migrations are immutable once written; change the schema with a new file.

CREATE TABLE acme_accounts (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    server     TEXT    NOT NULL CHECK (server IN ('letsencrypt', 'letsencrypt_staging')),
    email      TEXT    NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE acme_challenges (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    method     TEXT    NOT NULL CHECK (method IN ('dns', 'http')),
    -- acme.sh's DNS API script, e.g. dns_ionos; NULL for HTTP.
    provider   TEXT,
    -- A JSON object of the provider's variables and their values.
    credentials TEXT   NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    CHECK ((method = 'dns') = (provider IS NOT NULL))
) STRICT;

CREATE TABLE certificates (
    id              INTEGER PRIMARY KEY,
    -- A JSON array of the names; the first is the common name.
    domains         TEXT    NOT NULL,
    key_type        TEXT    NOT NULL CHECK (key_type IN ('ec256', 'rsa2048', 'rsa4096')),
    account_id      INTEGER NOT NULL REFERENCES acme_accounts (id),
    challenge_id    INTEGER NOT NULL REFERENCES acme_challenges (id),
    auto_renew      INTEGER NOT NULL CHECK (auto_renew IN (0, 1)),
    created_at      INTEGER NOT NULL,
    -- The last issuance that succeeded, and until when that certificate holds.
    issued_at       INTEGER,
    not_after       INTEGER,
    -- The last try, what went wrong (NULL: nothing) and what acme.sh wrote
    -- (a JSON array of lines).
    last_attempt_at INTEGER,
    last_error      TEXT,
    last_output     TEXT    NOT NULL DEFAULT '[]'
) STRICT;

-- One row: where nginx serves the web interface. No row: plain HTTP on 80
-- pointing to HTTPS on 443, with the machine's own certificate.
CREATE TABLE web_server (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    -- NULL: plain HTTP is not served.
    http_port      INTEGER CHECK (http_port BETWEEN 1 AND 65535),
    https_port     INTEGER NOT NULL CHECK (https_port BETWEEN 1 AND 65535),
    -- NULL: the machine's own, self-signed certificate.
    certificate_id INTEGER REFERENCES certificates (id)
) STRICT;
