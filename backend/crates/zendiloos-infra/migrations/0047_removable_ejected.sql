-- Removable drives an administrator ejected without powering them off
-- (zendiloos-storage): not mounted again by themselves, also after a
-- restart of zendiloosd or the agent, until they are pulled out or plugged
-- in anew.
CREATE TABLE removable_ejected (
    -- The drive's id, its file system's UUID.
    id         TEXT PRIMARY KEY,
    -- Which plugging in of its disk it was ejected in: the boot and the
    -- kernel's sequence number of the disk; NULL where udev did not say.
    plugged_in TEXT
) STRICT;
