//! Personal folders: switched on for a pool, then given to accounts one by
//! one. Turning them off, or taking one from an account, leaves the files
//! where they are; nobody reaches them until it is turned on again.

use zendiloos_kernel::{Actor, Outcome, RequestContext};

use super::SharingUseCases;
use super::shares::SharingError;
use crate::domain::{Home, HomesLocation, MIN_QUOTA_BYTES};

/// Where the personal folders are, and who has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomesSettings {
    /// `None` while personal folders are off.
    pub location: Option<HomesLocation>,
    pub homes: Vec<Home>,
}

impl SharingUseCases {
    pub async fn homes_settings(&self) -> Result<HomesSettings, SharingError> {
        Ok(HomesSettings {
            location: self.ports.shares.homes_location().await?,
            homes: self.ports.shares.homes().await?,
        })
    }

    /// Puts the personal folders on `pool` and offers them: the folders of
    /// accounts that have one are made there if missing.
    pub async fn enable_homes(
        &self,
        pool: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<HomesSettings, SharingError> {
        let _changes = self.changes.lock().await;
        let volumes = self.ports.volumes.volumes(request).await?;
        let volume =
            volumes.iter().find(|v| v.pool == pool && v.mounted).ok_or(SharingError::VolumeNotFound)?;
        // Older shares may be called `home`, from before the name was taken.
        if self.ports.shares.list().await?.iter().any(|share| share.name.as_str() == "home") {
            return Err(SharingError::Refused("a share is called home; rename or delete it first".into()));
        }
        let location = HomesLocation { pool: pool.to_owned(), volume: volume.number };
        self.ports.host.prepare_homes(&location.pool, location.volume, request).await?;
        let names = self.ports.directory.names().await?;
        for home in self.ports.shares.homes().await? {
            if let Some(name) = names.accounts.get(&home.account) {
                self.ports
                    .host
                    .set_home(&location.pool, location.volume, name, home.quota_bytes, request)
                    .await?;
            }
        }
        self.ports.shares.set_homes_location(Some(&location)).await?;
        self.apply_locked(&[], request).await?;
        self.audit("sharing.homes_enable", Outcome::Success, pool, actor, request).await;
        Ok(HomesSettings { location: Some(location), homes: self.ports.shares.homes().await? })
    }

    /// Stops offering the personal folders; their files stay.
    pub async fn disable_homes(&self, actor: &Actor, request: &RequestContext) -> Result<(), SharingError> {
        let _changes = self.changes.lock().await;
        self.ports.shares.set_homes_location(None).await?;
        self.apply_locked(&[], request).await?;
        self.audit("sharing.homes_disable", Outcome::Success, "", actor, request).await;
        Ok(())
    }

    /// The account's personal folder, if it has one.
    pub async fn home_of(&self, account: i64) -> Result<Option<Home>, SharingError> {
        Ok(self.ports.shares.homes().await?.into_iter().find(|home| home.account == account))
    }

    /// Gives `account` a personal folder holding at most `quota_bytes`
    /// (`None`: no limit), or changes the limit; with `enabled` false,
    /// takes it away, keeping its files.
    pub async fn set_account_home(
        &self,
        account: i64,
        enabled: bool,
        quota_bytes: Option<u64>,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Option<Home>, SharingError> {
        if quota_bytes.is_some_and(|bytes| bytes < MIN_QUOTA_BYTES) {
            return Err(SharingError::InvalidSettings(crate::domain::InvalidSettings::QuotaTooSmall));
        }
        let _changes = self.changes.lock().await;
        let names = self.ports.directory.names().await?;
        let name = names.accounts.get(&account).ok_or(SharingError::UnknownSubject)?.clone();
        if !enabled {
            self.ports.shares.remove_home(account).await?;
            self.apply_locked(&[], request).await?;
            self.audit("sharing.home_remove", Outcome::Success, &name, actor, request).await;
            return Ok(None);
        }
        let location = self
            .ports
            .shares
            .homes_location()
            .await?
            .ok_or_else(|| SharingError::Refused("personal folders are off".into()))?;
        self.ports.host.set_home(&location.pool, location.volume, &name, quota_bytes, request).await?;
        let home = Home { account, quota_bytes };
        self.ports.shares.set_home(&home).await?;
        self.apply_locked(&[], request).await?;
        self.audit("sharing.home_set", Outcome::Success, &name, actor, request).await;
        Ok(Some(home))
    }
}
