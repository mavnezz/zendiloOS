//! The host's local calendar, from the shared kernel: backups of volumes
//! and image updates are planned in it, as other contexts' schedules are.

pub use zendiloos_kernel::calendar::{UtcOffsets, date, day_number, local, moment, month_beside, weekday};
