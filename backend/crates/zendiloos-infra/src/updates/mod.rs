//! Updates: zendiloOS's releases from GitHub, and what changed in the
//! version that runs.

mod github;
mod notes;

pub use github::GitHubReleases;
pub use notes::installed_notes;
