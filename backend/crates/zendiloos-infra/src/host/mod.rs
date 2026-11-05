//! Unprivileged reads from the running Linux host (`/proc`, `/sys`,
//! `/etc/os-release`).

mod os_release;
mod system_info;

pub use system_info::HostSystemInfo;
