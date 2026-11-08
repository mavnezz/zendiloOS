-- Where each disk sits (zendiloos-storage): the enclosure chosen, and the
-- disks the NAS has seen.

-- One row. No row: not set up.
CREATE TABLE enclosure (
    id    INTEGER PRIMARY KEY CHECK (id = 1),
    -- A model of the built-in catalog; NULL for a layout of one's own,
    -- whose groups and slots follow.
    model TEXT
) STRICT;

-- The groups of a layout of one's own, in their order.
CREATE TABLE enclosure_groups (
    position     INTEGER PRIMARY KEY CHECK (position >= 0),
    id           TEXT    NOT NULL UNIQUE,
    name         TEXT    NOT NULL,
    row_count    INTEGER NOT NULL CHECK (row_count >= 1),
    column_count INTEGER NOT NULL CHECK (column_count >= 1),
    orientation  TEXT    NOT NULL CHECK (orientation IN ('vertical', 'horizontal')),
    form         TEXT    NOT NULL CHECK (form IN ('inch_3_5', 'inch_2_5', 'm2')),
    numbering    TEXT    NOT NULL
        CHECK (numbering IN ('left_to_right', 'right_to_left', 'top_to_bottom', 'bottom_to_top')),
    CHECK (row_count * column_count <= 60)
) STRICT;

-- Their slots, in the order their numbering fills the grid.
CREATE TABLE enclosure_slots (
    group_id TEXT    NOT NULL REFERENCES enclosure_groups (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    id       TEXT    NOT NULL UNIQUE,
    label    TEXT    NOT NULL,
    PRIMARY KEY (group_id, position)
) STRICT;

-- What a slot is bound to: in a layout of one's own, or instead of what a
-- model binds there (kind NULL: nothing).
CREATE TABLE enclosure_bindings (
    slot  TEXT PRIMARY KEY,
    -- 'port': udev's ID_PATH of a whole disk; 'serial': a serial number.
    kind  TEXT CHECK (kind IN ('port', 'serial')),
    value TEXT,
    CHECK ((kind IS NULL) = (value IS NULL))
) STRICT;

-- Every disk seen, by serial number, with what the owner noted about it.
CREATE TABLE disk_inventory (
    serial         TEXT    PRIMARY KEY,
    -- As last seen.
    model          TEXT,
    size_bytes     INTEGER NOT NULL CHECK (size_bytes >= 0),
    -- Days, 'YYYY-MM-DD'.
    purchased      TEXT,
    warranty_until TEXT,
    -- Where it was bought.
    vendor         TEXT,
    note           TEXT,
    first_seen     INTEGER NOT NULL,
    last_seen      INTEGER NOT NULL
) STRICT;
