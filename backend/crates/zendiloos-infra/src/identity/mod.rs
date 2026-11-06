//! Adapters for the ports of the Identity context, except the database
//! repositories, which live in [`crate::sqlite`]; and the bridge that lets
//! the Sharing context follow groups.

mod groups;
mod password;
mod setup_code;
mod tokens;

pub use groups::SharingFollowsGroups;
pub use password::Argon2PasswordHasher;
pub use setup_code::FileSetupCodes;
pub use tokens::RandomSessionTokens;
