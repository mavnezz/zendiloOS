-- Serial numbers two disks were seen reporting at once (zendiloos-storage),
-- as the disks of some USB docks report their bridge's. Their disks are
-- kept by serial number and port from then on, also while one of them is
-- there alone: a disk's record does not change with which others are
-- present.
CREATE TABLE disk_shared_serials (
    serial TEXT PRIMARY KEY
) STRICT;

-- The inventory kept such disks as `SERIAL@ID_PATH` while both were there.
INSERT INTO disk_shared_serials (serial)
SELECT DISTINCT substr(serial, 1, instr(serial, '@') - 1)
FROM disk_inventory
WHERE instr(serial, '@') > 1;

-- A record by the serial number alone was of whichever of them was there
-- alone, and is never seen again. It goes where the owner noted nothing
-- about it; where they did, it stays, to be moved over and forgotten by
-- hand.
DELETE FROM disk_inventory
WHERE serial IN (SELECT serial FROM disk_shared_serials)
    AND purchased IS NULL
    AND warranty_until IS NULL
    AND vendor IS NULL
    AND note IS NULL;
