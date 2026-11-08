use std::collections::HashMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_sharing::application::ports::{Directory, Names, PendingUpload, ShareRepository};
use zendiloos_sharing::domain::{
    Access, AccessRule, AppAccess, Compression, Favourite, FileLocation, FilePath, FileRoot, FileServices,
    Frequency, Home, HomesLocation, NewShare, NfsClient, NfsSquash, Permission, REMOVABLE_PREFIX, RecordSize,
    Share, ShareId, ShareName, ShareSettings, SmbEncryption, SmbProtocol, SmbSigning, SnapshotSchedule,
    Subject, TimeMachine, is_drive_id,
};

use super::Database;

/// [`ShareRepository`] on the `shares` tables, and the [`Directory`] of
/// account and group names.
#[derive(Debug, Clone)]
pub struct SqliteShares {
    db: Database,
}

impl SqliteShares {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("shares", error)
}

fn stored(what: impl Into<String>) -> PortError {
    PortError::new("stored share", what.into())
}

struct ShareRow {
    id: i64,
    name: String,
    description: Option<String>,
    pool: String,
    volume: i64,
    shared: bool,
    recycle_bin: bool,
    quota_bytes: Option<i64>,
    compression: String,
    record_size: i64,
    created_at: i64,
    updated_at: i64,
}

fn columns(settings: &ShareSettings) -> Result<(Option<i64>, &'static str, i64), PortError> {
    let quota = settings.quota_bytes.map(i64::try_from).transpose().map_err(|_| stored("quota"))?;
    let record_size = i64::try_from(settings.record_size.bytes()).map_err(|_| stored("record size"))?;
    Ok((quota, settings.compression.as_str(), record_size))
}

fn settings(row: &ShareRow) -> Result<ShareSettings, PortError> {
    Ok(ShareSettings {
        quota_bytes: row.quota_bytes.map(u64::try_from).transpose().map_err(|_| stored("quota"))?,
        compression: Compression::parse(&row.compression).ok_or_else(|| stored("compression"))?,
        record_size: u64::try_from(row.record_size)
            .ok()
            .and_then(RecordSize::from_bytes)
            .ok_or_else(|| stored("record size"))?,
    })
}

async fn replace_rules(
    tx: &mut sqlx::SqliteConnection,
    id: i64,
    access: &[AccessRule],
    nfs: &[NfsClient],
) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM share_access WHERE share_id = ?", id).execute(&mut *tx).await?;
    sqlx::query!("DELETE FROM share_nfs_clients WHERE share_id = ?", id).execute(&mut *tx).await?;
    for (position, rule) in access.iter().enumerate() {
        let (account, group) = match rule.subject {
            Subject::Account(account) => (Some(account), None),
            Subject::Group(group) => (None, Some(group)),
        };
        let level = match rule.permission {
            Permission::Deny => "deny",
            Permission::Read => "read",
            Permission::ReadWrite => "read_write",
        };
        let position = position as i64;
        sqlx::query!(
            "INSERT INTO share_access (share_id, account_id, group_id, access, position) VALUES (?, ?, ?, ?, ?)",
            id,
            account,
            group,
            level,
            position
        )
        .execute(&mut *tx)
        .await?;
    }
    for (position, client) in nfs.iter().enumerate() {
        let position = position as i64;
        let squash = client.squash.as_str();
        sqlx::query!(
            "INSERT INTO share_nfs_clients (share_id, network, write, squash, async, insecure, position)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            id,
            client.network,
            client.write,
            squash,
            client.asynchronous,
            client.insecure,
            position
        )
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

impl SqliteShares {
    async fn load(&self, rows: Vec<ShareRow>) -> Result<Vec<Share>, PortError> {
        let rules = sqlx::query!(
            r#"SELECT share_id AS "share_id!", account_id, group_id, access FROM share_access ORDER BY position"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let clients = sqlx::query!(
            r#"SELECT share_id AS "share_id!", network AS "network!", write AS "write!: bool",
                      squash AS "squash!", async AS "asynchronous!: bool", insecure AS "insecure!: bool"
               FROM share_nfs_clients ORDER BY position"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let mut access: HashMap<i64, Vec<AccessRule>> = HashMap::new();
        for rule in rules {
            let subject = match (rule.account_id, rule.group_id) {
                (Some(account), None) => Subject::Account(account),
                (None, Some(group)) => Subject::Group(group),
                _ => return Err(stored("a rule without exactly one subject")),
            };
            let permission = match rule.access.as_str() {
                "deny" => Permission::Deny,
                "read" => Permission::Read,
                "read_write" => Permission::ReadWrite,
                other => return Err(stored(format!("access {other:?}"))),
            };
            access.entry(rule.share_id).or_default().push(AccessRule { subject, permission });
        }
        let mut nfs: HashMap<i64, Vec<NfsClient>> = HashMap::new();
        for client in clients {
            let squash = NfsSquash::parse(&client.squash).ok_or_else(|| stored("NFS squash"))?;
            nfs.entry(client.share_id).or_default().push(NfsClient {
                network: client.network,
                write: client.write,
                squash,
                asynchronous: client.asynchronous,
                insecure: client.insecure,
            });
        }
        rows.into_iter()
            .map(|row| {
                let settings = settings(&row)?;
                Ok(Share {
                    id: ShareId::new(row.id),
                    name: ShareName::parse(&row.name).map_err(|e| stored(e.to_string()))?,
                    description: row.description,
                    pool: row.pool,
                    volume: u16::try_from(row.volume).map_err(|_| stored("volume"))?,
                    shared: row.shared,
                    recycle_bin: row.recycle_bin,
                    settings,
                    access: access.remove(&row.id).unwrap_or_default(),
                    nfs: nfs.remove(&row.id).unwrap_or_default(),
                    created_at: Timestamp::from_unix_seconds(row.created_at),
                    updated_at: Timestamp::from_unix_seconds(row.updated_at),
                })
            })
            .collect()
    }
}

#[async_trait]
impl ShareRepository for SqliteShares {
    async fn list(&self) -> Result<Vec<Share>, PortError> {
        let rows = sqlx::query_as!(
            ShareRow,
            r#"SELECT id AS "id!", name, description, pool, volume, shared AS "shared: bool",
                      recycle_bin AS "recycle_bin: bool",
                      quota_bytes, compression, record_size, created_at, updated_at
               FROM shares WHERE deleting = 0 ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        self.load(rows).await
    }

    async fn set_deleting(&self, id: ShareId, deleting: bool) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("UPDATE shares SET deleting = ? WHERE id = ?", deleting, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn deleting(&self) -> Result<Vec<Share>, PortError> {
        let rows = sqlx::query_as!(
            ShareRow,
            r#"SELECT id AS "id!", name, description, pool, volume, shared AS "shared: bool",
                      recycle_bin AS "recycle_bin: bool",
                      quota_bytes, compression, record_size, created_at, updated_at
               FROM shares WHERE deleting = 1 ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        self.load(rows).await
    }

    async fn find(&self, id: ShareId) -> Result<Option<Share>, PortError> {
        Ok(self.list().await?.into_iter().find(|share| share.id == id))
    }

    async fn add_upload(&self, user: &str, at: &FileLocation, started: Timestamp) -> Result<i64, PortError> {
        // A removable drive as `removable:<id>`: a share's name has no `:`.
        let share = match &at.root {
            FileRoot::Share(share) => share.as_str().to_owned(),
            FileRoot::Home => String::new(),
            FileRoot::Removable(_) => at.root.to_string(),
        };
        let result = sqlx::query(
            "INSERT INTO pending_uploads (user, volume, share, path, started_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(user)
        .bind(i64::from(at.volume))
        .bind(share)
        .bind(at.path.as_str())
        .bind(started.unix_seconds())
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(result.last_insert_rowid())
    }

    async fn remove_upload(&self, id: i64) -> Result<(), PortError> {
        sqlx::query("DELETE FROM pending_uploads WHERE id = ?")
            .bind(id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn uploads_before(&self, before: Timestamp) -> Result<Vec<PendingUpload>, PortError> {
        let rows: Vec<(i64, String, i64, String, String)> = sqlx::query_as(
            "SELECT id, user, volume, share, path FROM pending_uploads WHERE started_at < ? ORDER BY id",
        )
        .bind(before.unix_seconds())
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|(id, user, volume, share, path)| {
                let root = if share.is_empty() {
                    FileRoot::Home
                } else if let Some(drive) = share.strip_prefix(REMOVABLE_PREFIX) {
                    if !is_drive_id(drive) {
                        return Err(stored(format!("upload {id}: drive")));
                    }
                    FileRoot::Removable(drive.to_owned())
                } else {
                    FileRoot::Share(
                        ShareName::parse(&share).map_err(|_| stored(format!("upload {id}: share")))?,
                    )
                };
                let at = FileLocation {
                    volume: u16::try_from(volume).map_err(|_| stored(format!("upload {id}: volume")))?,
                    root,
                    path: FilePath::parse(&path).map_err(|_| stored(format!("upload {id}: path")))?,
                    snapshot: None,
                };
                Ok(PendingUpload { id, user, at })
            })
            .collect()
    }

    async fn create(&self, new: NewShare) -> Result<Option<Share>, PortError> {
        let (name, at, volume) = (new.name.as_str(), new.created_at.unix_seconds(), i64::from(new.volume));
        let (quota, compression, record_size) = columns(&new.settings)?;
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        let id = sqlx::query_scalar!(
            r#"INSERT INTO shares (name, description, pool, volume, recycle_bin, quota_bytes, compression, record_size,
                                   created_at, updated_at, shared)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)
               ON CONFLICT (name) DO NOTHING
               RETURNING id AS "id!""#,
            name,
            new.description,
            new.pool,
            volume,
            new.recycle_bin,
            quota,
            compression,
            record_size,
            at,
            new.shared
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(id) = id else { return Ok(None) };
        replace_rules(&mut tx, id, &new.access, &new.nfs).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(Some(Share {
            id: ShareId::new(id),
            name: new.name,
            description: new.description,
            pool: new.pool,
            volume: new.volume,
            shared: new.shared,
            recycle_bin: new.recycle_bin,
            settings: new.settings,
            access: new.access,
            nfs: new.nfs,
            created_at: new.created_at,
            updated_at: new.created_at,
        }))
    }

    async fn update(&self, share: &Share) -> Result<(), PortError> {
        let (id, at) = (share.id.get(), share.updated_at.unix_seconds());
        let (quota, compression, record_size) = columns(&share.settings)?;
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!(
            "UPDATE shares SET description = ?, recycle_bin = ?, quota_bytes = ?, compression = ?, record_size = ?,
                               updated_at = ?, shared = ?
             WHERE id = ?",
            share.description,
            share.recycle_bin,
            quota,
            compression,
            record_size,
            at,
            share.shared,
            id
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        replace_rules(&mut tx, id, &share.access, &share.nfs).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }

    async fn delete(&self, id: ShareId) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("DELETE FROM shares WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn owe_access(&self, shares: &[ShareId]) -> Result<(), PortError> {
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        for id in shares.iter().map(|id| id.get()) {
            sqlx::query!("UPDATE shares SET access_owed = access_owed + 1 WHERE id = ?", id)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)
    }

    async fn access_owed(&self) -> Result<Vec<(ShareId, u64)>, PortError> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", access_owed FROM shares
               WHERE deleting = 0 AND access_given < access_owed ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let mark = u64::try_from(row.access_owed).map_err(|_| stored("owed access"))?;
                Ok((ShareId::new(row.id), mark))
            })
            .collect()
    }

    async fn access_settled(&self, share: ShareId, mark: u64) -> Result<(), PortError> {
        let (id, mark) = (share.get(), i64::try_from(mark).map_err(|_| stored("owed access"))?);
        sqlx::query!("UPDATE shares SET access_given = MAX(access_given, ?) WHERE id = ?", mark, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn snapshot_schedules(&self, id: ShareId) -> Result<Vec<SnapshotSchedule>, PortError> {
        let id = id.get();
        let rows =
            sqlx::query!("SELECT frequency, keep FROM share_snapshot_schedules WHERE share_id = ?", id)
                .fetch_all(self.db.pool())
                .await
                .map_err(db_error)?;
        let mut schedules: Vec<SnapshotSchedule> = rows
            .into_iter()
            .filter_map(|row| {
                Some(SnapshotSchedule {
                    frequency: Frequency::parse(&row.frequency)?,
                    keep: u32::try_from(row.keep).ok()?,
                })
            })
            .collect();
        schedules.sort_by_key(|schedule| schedule.frequency);
        Ok(schedules)
    }

    async fn set_snapshot_schedules(
        &self,
        id: ShareId,
        schedules: &[SnapshotSchedule],
    ) -> Result<(), PortError> {
        let id = id.get();
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM share_snapshot_schedules WHERE share_id = ?", id)
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        for schedule in schedules {
            let (frequency, keep) = (schedule.frequency.as_str(), i64::from(schedule.keep));
            sqlx::query!(
                "INSERT INTO share_snapshot_schedules (share_id, frequency, keep) VALUES (?, ?, ?)",
                id,
                frequency,
                keep
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)
    }

    async fn app_access(&self) -> Result<Vec<AppAccess>, PortError> {
        let rows =
            sqlx::query!("SELECT share_id, account, access FROM share_app_access ORDER BY share_id, account")
                .fetch_all(self.db.pool())
                .await
                .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let access = match row.access.as_str() {
                    "read" => Access::Read,
                    "read_write" => Access::ReadWrite,
                    other => return Err(stored(format!("access {other:?}"))),
                };
                Ok(AppAccess { share: ShareId::new(row.share_id), account: row.account, access })
            })
            .collect()
    }

    async fn set_app_access(&self, account: &str, access: &[(ShareId, Access)]) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM share_app_access WHERE account = ?", account)
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        for (share, level) in access {
            let share = share.get();
            let level = match level {
                Access::Read => "read",
                Access::ReadWrite => "read_write",
            };
            sqlx::query!(
                "INSERT INTO share_app_access (share_id, account, access) VALUES (?, ?, ?)",
                share,
                account,
                level
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)
    }

    async fn file_services(&self) -> Result<FileServices, PortError> {
        let row = sqlx::query!(
            r#"SELECT smb AS "smb: bool", nfs AS "nfs: bool", workgroup, smb_min_protocol, smb_encryption,
                      smb_signing, hide_unreadable AS "hide_unreadable: bool", multichannel AS "multichannel: bool",
                      oplocks AS "oplocks: bool", leases AS "leases: bool",
                      durable_handles AS "durable_handles: bool", mac_compat AS "mac_compat: bool",
                      follow_symlinks AS "follow_symlinks: bool", veto_files,
                      delete_veto_files AS "delete_veto_files: bool", wsdd AS "wsdd: bool", bonjour AS "bonjour: bool",
                      time_machine_share, time_machine_max_bytes
               FROM file_services WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else { return Ok(FileServices::default()) };
        let stored = |what: &str| PortError::new("stored file services", what.to_owned());
        Ok(FileServices {
            smb: row.smb,
            nfs: row.nfs,
            workgroup: row.workgroup,
            smb_min_protocol: SmbProtocol::parse(&row.smb_min_protocol)
                .ok_or_else(|| stored("SMB version"))?,
            smb_encryption: SmbEncryption::parse(&row.smb_encryption).ok_or_else(|| stored("encryption"))?,
            smb_signing: SmbSigning::parse(&row.smb_signing).ok_or_else(|| stored("signing"))?,
            hide_unreadable: row.hide_unreadable,
            multichannel: row.multichannel,
            oplocks: row.oplocks,
            leases: row.leases,
            durable_handles: row.durable_handles,
            mac_compat: row.mac_compat,
            follow_symlinks: row.follow_symlinks,
            veto_files: row.veto_files,
            delete_veto_files: row.delete_veto_files,
            wsdd: row.wsdd,
            bonjour: row.bonjour,
            time_machine: match row.time_machine_share {
                Some(share) => Some(TimeMachine {
                    share: ShareId::new(share),
                    max_bytes: row
                        .time_machine_max_bytes
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| stored("Time Machine size"))?,
                }),
                None => None,
            },
        })
    }

    async fn set_file_services(&self, services: &FileServices) -> Result<(), PortError> {
        let (protocol, encryption, signing) = (
            services.smb_min_protocol.as_str(),
            services.smb_encryption.as_str(),
            services.smb_signing.as_str(),
        );
        let machine_share = services.time_machine.map(|machine| machine.share.get());
        let machine_bytes = services
            .time_machine
            .and_then(|machine| machine.max_bytes)
            .map(i64::try_from)
            .transpose()
            .map_err(|_| stored("Time Machine size"))?;
        sqlx::query!(
            "INSERT INTO file_services (id, smb, nfs, workgroup, smb_min_protocol, smb_encryption, smb_signing,
                 hide_unreadable, multichannel, oplocks, leases, durable_handles, mac_compat, follow_symlinks,
                 veto_files, delete_veto_files, wsdd, bonjour, time_machine_share, time_machine_max_bytes)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET smb = excluded.smb, nfs = excluded.nfs,
                 workgroup = excluded.workgroup, smb_min_protocol = excluded.smb_min_protocol,
                 smb_encryption = excluded.smb_encryption, smb_signing = excluded.smb_signing,
                 hide_unreadable = excluded.hide_unreadable, multichannel = excluded.multichannel,
                 oplocks = excluded.oplocks, leases = excluded.leases, durable_handles = excluded.durable_handles,
                 mac_compat = excluded.mac_compat, follow_symlinks = excluded.follow_symlinks,
                 veto_files = excluded.veto_files, delete_veto_files = excluded.delete_veto_files,
                 wsdd = excluded.wsdd, bonjour = excluded.bonjour, time_machine_share = excluded.time_machine_share,
                 time_machine_max_bytes = excluded.time_machine_max_bytes",
            services.smb,
            services.nfs,
            services.workgroup,
            protocol,
            encryption,
            signing,
            services.hide_unreadable,
            services.multichannel,
            services.oplocks,
            services.leases,
            services.durable_handles,
            services.mac_compat,
            services.follow_symlinks,
            services.veto_files,
            services.delete_veto_files,
            services.wsdd,
            services.bonjour,
            machine_share,
            machine_bytes
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn homes_location(&self) -> Result<Option<HomesLocation>, PortError> {
        let row = sqlx::query!("SELECT pool, volume FROM homes_location WHERE id = 1")
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        row.map(|row| {
            let volume = u16::try_from(row.volume).map_err(|_| stored("volume of the personal folders"))?;
            Ok(HomesLocation { pool: row.pool, volume })
        })
        .transpose()
    }

    async fn set_homes_location(&self, location: Option<&HomesLocation>) -> Result<(), PortError> {
        match location {
            Some(location) => {
                let volume = i64::from(location.volume);
                sqlx::query!(
                    "INSERT INTO homes_location (id, pool, volume) VALUES (1, ?, ?)
                     ON CONFLICT (id) DO UPDATE SET pool = excluded.pool, volume = excluded.volume",
                    location.pool,
                    volume
                )
                .execute(self.db.pool())
                .await
            }
            None => sqlx::query!("DELETE FROM homes_location").execute(self.db.pool()).await,
        }
        .map_err(db_error)?;
        Ok(())
    }

    async fn homes(&self) -> Result<Vec<Home>, PortError> {
        let rows =
            sqlx::query!(r#"SELECT account_id AS "account_id!", quota_bytes FROM homes ORDER BY account_id"#)
                .fetch_all(self.db.pool())
                .await
                .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let quota_bytes = row
                    .quota_bytes
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| stored("quota of a personal folder"))?;
                Ok(Home { account: row.account_id, quota_bytes })
            })
            .collect()
    }

    async fn set_home(&self, home: &Home) -> Result<(), PortError> {
        let quota = home.quota_bytes.map(i64::try_from).transpose().map_err(|_| stored("quota"))?;
        sqlx::query!(
            "INSERT INTO homes (account_id, quota_bytes) VALUES (?, ?)
             ON CONFLICT (account_id) DO UPDATE SET quota_bytes = excluded.quota_bytes",
            home.account,
            quota
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn remove_home(&self, account: i64) -> Result<(), PortError> {
        sqlx::query!("DELETE FROM homes WHERE account_id = ?", account)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn favourites(&self, account: i64) -> Result<Vec<Favourite>, PortError> {
        let rows = sqlx::query!(
            "SELECT share_id, path FROM file_favourites WHERE account_id = ? ORDER BY position",
            account
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(Favourite { share: ShareId::new(row.share_id), path: FilePath::parse(&row.path).ok()? })
            })
            .collect())
    }

    async fn set_favourites(&self, account: i64, favourites: &[Favourite]) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM file_favourites WHERE account_id = ?", account)
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        for (position, favourite) in favourites.iter().enumerate() {
            let (share, path) = (favourite.share.get(), favourite.path.as_str());
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            sqlx::query!(
                "INSERT INTO file_favourites (account_id, share_id, path, position) VALUES (?, ?, ?, ?)",
                account,
                share,
                path,
                position
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)
    }
}

#[async_trait]
impl Directory for SqliteShares {
    async fn names(&self) -> Result<Names, PortError> {
        let accounts =
            sqlx::query!(r#"SELECT id AS "id!", name, disabled AS "disabled: bool" FROM accounts"#)
                .fetch_all(self.db.pool())
                .await
                .map_err(db_error)?;
        let groups = sqlx::query!(r#"SELECT id AS "id!", name FROM groups"#)
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        let memberships = sqlx::query!(
            r#"SELECT group_id AS "group_id!", account_id AS "account_id!" FROM group_members
               ORDER BY group_id, account_id"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let mut members: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in memberships {
            members.entry(row.group_id).or_default().push(row.account_id);
        }
        Ok(Names {
            disabled: accounts.iter().filter(|a| a.disabled).map(|a| a.id).collect(),
            accounts: accounts.into_iter().map(|a| (a.id, a.name)).collect(),
            groups: groups.into_iter().map(|g| (g.id, g.name)).collect(),
            members,
        })
    }

    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError> {
        sqlx::query_scalar!(
            r#"SELECT group_id AS "group_id!" FROM group_members WHERE account_id = ?"#,
            account
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_identity::application::ports::{AccountRepository, GroupRepository};
    use zendiloos_identity::domain::{AccountName, GroupName, NewAccount, NewGroup, PasswordHash, Role};

    use super::*;
    use crate::sqlite::tests::temporary_database;
    use crate::sqlite::{SqliteAccounts, SqliteGroups};

    #[tokio::test]
    async fn uploads_under_way_are_remembered_until_they_end() {
        let (_dir, db) = temporary_database().await;
        let shares = SqliteShares::new(db);
        let at = |root, path: &str| FileLocation {
            volume: 1,
            root,
            path: FilePath::parse(path).unwrap(),
            snapshot: None,
        };
        let photos = at(FileRoot::Share(ShareName::parse("photos").unwrap()), "2024/.zendiloos-upload-1");
        let home = at(FileRoot::Home, ".zendiloos-upload-2");
        let first = shares.add_upload("alice", &photos, Timestamp::from_unix_seconds(100)).await.unwrap();
        shares.add_upload("bob", &home, Timestamp::from_unix_seconds(200)).await.unwrap();

        let before = shares.uploads_before(Timestamp::from_unix_seconds(150)).await.unwrap();
        assert_eq!(before, [PendingUpload { id: first, user: "alice".into(), at: photos }]);
        let all = shares.uploads_before(Timestamp::from_unix_seconds(201)).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[1].user.as_str(), &all[1].at), ("bob", &home), "a personal folder");
        shares.remove_upload(first).await.unwrap();
        assert_eq!(shares.uploads_before(Timestamp::from_unix_seconds(201)).await.unwrap().len(), 1);

        let stick =
            FileLocation { volume: 0, ..at(FileRoot::Removable("1A2B-3C4D".into()), ".zendiloos-upload-3") };
        shares.add_upload("alice", &stick, Timestamp::from_unix_seconds(300)).await.unwrap();
        let all = shares.uploads_before(Timestamp::from_unix_seconds(301)).await.unwrap();
        assert_eq!(all.last().map(|upload| &upload.at), Some(&stick), "a removable drive");
    }

    #[tokio::test]
    async fn file_services_are_the_defaults_until_set() {
        let (_dir, db) = temporary_database().await;
        let shares = SqliteShares::new(db);
        assert_eq!(shares.file_services().await.unwrap(), FileServices::default());

        let set = FileServices {
            smb: false,
            nfs: true,
            workgroup: "HOME".into(),
            smb_min_protocol: SmbProtocol::Smb3,
            smb_signing: SmbSigning::Mandatory,
            veto_files: Some("/*.tmp/".into()),
            bonjour: false,
            time_machine: Some(TimeMachine { share: ShareId::new(7), max_bytes: Some(1 << 40) }),
            ..FileServices::default()
        };
        shares.set_file_services(&set).await.unwrap();
        shares.set_file_services(&set).await.unwrap();

        assert_eq!(shares.file_services().await.unwrap(), set);
    }

    #[tokio::test]
    async fn stores_shares_with_rules_that_follow_accounts_and_groups() {
        let (_dir, db) = temporary_database().await;
        let at = Timestamp::from_unix_seconds(100);
        let alice = SqliteAccounts::new(db.clone())
            .create(NewAccount {
                name: AccountName::parse("alice").unwrap(),
                full_name: None,
                description: None,
                email: None,
                role: Role::User,
                password_hash: PasswordHash::new("$argon2id$x"),
                password_change_required: false,
                password_locked: false,
                created_at: at,
            })
            .await
            .unwrap()
            .unwrap();
        let family = SqliteGroups::new(db.clone())
            .create(NewGroup {
                name: GroupName::parse("family").unwrap(),
                description: None,
                members: vec![alice.id],
                created_at: at,
            })
            .await
            .unwrap()
            .unwrap();
        let shares = SqliteShares::new(db.clone());
        let new = NewShare {
            name: ShareName::parse("photos").unwrap(),
            description: Some("Pictures".into()),
            pool: "tank".into(),
            volume: 1,
            shared: false,
            recycle_bin: true,
            access: vec![
                AccessRule { subject: Subject::Group(family.id.get()), permission: Permission::ReadWrite },
                AccessRule { subject: Subject::Account(alice.id.get()), permission: Permission::Read },
            ],
            nfs: vec![
                NfsClient::new("192.168.1.0/24", false),
                NfsClient {
                    squash: NfsSquash::All,
                    asynchronous: true,
                    insecure: true,
                    ..NfsClient::new("*.example.com", true)
                },
            ],
            settings: ShareSettings {
                quota_bytes: Some(5 << 30),
                compression: Compression::Zstd,
                record_size: RecordSize::M1,
            },
            created_at: at,
        };
        let mut share = shares.create(new.clone()).await.unwrap().unwrap();
        assert_eq!(shares.create(new).await.unwrap(), None, "name taken");
        assert_eq!(shares.find(share.id).await.unwrap(), Some(share.clone()));
        let names = shares.names().await.unwrap();
        assert_eq!(names.accounts[&alice.id.get()], "alice");
        assert_eq!(names.groups[&family.id.get()], "family");
        assert_eq!(shares.memberships(alice.id.get()).await.unwrap(), [family.id.get()]);
        assert!(shares.memberships(alice.id.get() + 1).await.unwrap().is_empty());

        assert!(!share.shared, "kept off the network");
        share.nfs.clear();
        share.shared = true;
        share.recycle_bin = false;
        share.settings = ShareSettings::default();
        shares.update(&share).await.unwrap();
        assert_eq!(shares.list().await.unwrap(), [share.clone()]);

        let favourite = |path: &str| Favourite { share: share.id, path: FilePath::parse(path).unwrap() };
        let mine = [favourite("b"), favourite("a"), favourite("")];
        shares.set_favourites(alice.id.get(), &mine).await.unwrap();
        assert_eq!(shares.favourites(alice.id.get()).await.unwrap(), mine, "in the order given");
        shares.set_favourites(alice.id.get(), &mine[1..]).await.unwrap();
        assert_eq!(shares.favourites(alice.id.get()).await.unwrap(), mine[1..]);

        SqliteAccounts::new(db.clone()).delete(alice.id).await.unwrap();
        assert!(shares.favourites(alice.id.get()).await.unwrap().is_empty(), "they go with the account");
        let rules = shares.find(share.id).await.unwrap().unwrap().access;
        assert_eq!(
            rules,
            [AccessRule { subject: Subject::Group(family.id.get()), permission: Permission::ReadWrite }]
        );

        let plans = [
            SnapshotSchedule { frequency: Frequency::Daily, keep: 7 },
            SnapshotSchedule { frequency: Frequency::Hourly, keep: 24 },
        ];
        shares.set_snapshot_schedules(share.id, &plans).await.unwrap();
        assert_eq!(shares.snapshot_schedules(share.id).await.unwrap(), [plans[1], plans[0]]);
        shares.set_snapshot_schedules(share.id, &plans[..1]).await.unwrap();
        assert_eq!(shares.snapshot_schedules(share.id).await.unwrap(), [plans[0]]);

        shares.delete(share.id).await.unwrap();
        assert!(shares.list().await.unwrap().is_empty());
        assert!(shares.snapshot_schedules(share.id).await.unwrap().is_empty(), "they go with the share");
        db.close().await;
    }

    fn share_named(name: &str) -> NewShare {
        NewShare {
            name: ShareName::parse(name).unwrap(),
            description: None,
            pool: "tank".into(),
            volume: 1,
            shared: true,
            recycle_bin: true,
            access: vec![],
            nfs: vec![],
            settings: ShareSettings::default(),
            created_at: Timestamp::from_unix_seconds(100),
        }
    }

    #[tokio::test]
    async fn what_a_shares_files_owe_is_settled_only_up_to_the_mark_a_walk_read() {
        let (_dir, db) = temporary_database().await;
        let shares = SqliteShares::new(db.clone());
        let photos = shares.create(share_named("photos")).await.unwrap().unwrap().id;
        let media = shares.create(share_named("media")).await.unwrap().unwrap().id;
        assert!(shares.access_owed().await.unwrap().is_empty(), "a new share owes nothing");

        shares.owe_access(&[photos, media]).await.unwrap();
        assert_eq!(shares.access_owed().await.unwrap(), [(media, 1), (photos, 1)]);
        // A walk of photos read mark 1; meanwhile its rules changed again.
        shares.owe_access(&[photos]).await.unwrap();
        shares.access_settled(photos, 1).await.unwrap();
        shares.access_settled(media, 1).await.unwrap();
        assert_eq!(shares.access_owed().await.unwrap(), [(photos, 2)], "the later mark stays");
        shares.access_settled(photos, 2).await.unwrap();
        shares.access_settled(photos, 1).await.unwrap();
        assert!(shares.access_owed().await.unwrap().is_empty(), "an older walk takes nothing back");

        shares.owe_access(&[media]).await.unwrap();
        shares.set_deleting(media, true).await.unwrap();
        assert!(shares.access_owed().await.unwrap().is_empty(), "a share being deleted is walked no more");
        db.close().await;
    }

    #[tokio::test]
    async fn disabled_accounts_are_named_as_such() {
        let (_dir, db) = temporary_database().await;
        let accounts = SqliteAccounts::new(db.clone());
        let new = |name: &str| NewAccount {
            name: AccountName::parse(name).unwrap(),
            full_name: None,
            description: None,
            email: None,
            role: Role::User,
            password_hash: PasswordHash::new("$argon2id$x"),
            password_change_required: false,
            password_locked: false,
            created_at: Timestamp::from_unix_seconds(100),
        };
        let alice = accounts.create(new("alice")).await.unwrap().unwrap();
        let mut bob = accounts.create(new("bob")).await.unwrap().unwrap();
        bob.disabled = true;
        accounts.update(&bob).await.unwrap();

        let names = SqliteShares::new(db.clone()).names().await.unwrap();
        assert_eq!(names.accounts.len(), 2);
        assert_eq!(names.disabled, [bob.id.get()].into_iter().collect());
        assert!(!names.disabled.contains(&alice.id.get()));
        db.close().await;
    }
}
