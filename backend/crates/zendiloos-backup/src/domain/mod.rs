//! The rules of backup tasks: their places and settings, schedules, how a
//! backup looks on its target and what retention keeps, and the log.

mod runs;
mod schedule;
mod task;
mod versions;

pub use runs::{
    DEFAULT_LOG_DAYS, Existing, InvalidSettings, MAX_LOG_DAYS, Outcome, Run, Settings, TRANSFERS_KEPT,
    Transfer, Trigger,
};
pub use schedule::{Frequency, InvalidSchedule, Schedule};
pub use task::{
    DEFAULT_EXCLUDES, Filters, Invalid, MAX_BWLIMIT_KIB, MAX_EXCLUDES, MAX_KEEP_DAYS, MAX_KEEP_VERSIONS,
    MAX_NAME, Mode, NasPlace, Place, RemotePlace, Server, Task, TaskKind, TaskSettings, Transport,
    folder_name, is_drive_id, is_folder_path, is_host, is_host_key, is_name, is_pattern, is_remote_path,
    is_word,
};
pub use versions::{
    CURRENT, Entry, EntryKind, INCOMPLETE, MARKER, REMOVED, TargetState, beyond_retention, parse_version,
    removed_beyond, version_name,
};
