use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ErrorCode, GroupName, NfsClient, NfsExport, Request, Response,
    ShareAccess, ShareComment, ShareCompression, ShareRecordSize, SmbShare, SnapshotFrequency, SnapshotLabel,
    SnapshotNote, UserName,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_sharing::application::ports::{HostError, ShareHost, Volume, Volumes};
use zendiloos_sharing::domain::{
    Compression, Frequency, Permission, Principal, RecordSize, ShareName, ShareSettings, ShareUsage,
    SharingConfig, SmbEncryption, SmbProtocol, SmbSigning, Snapshot, is_label,
};
use zendiloos_storage::application::ports::ZfsSource;

use super::{WHAT, agent_error, call_raw, unexpected};
use crate::agent::AgentStorage;

/// [`ShareHost`] through the agents: datasets and server configuration
/// through the system agent, directory permissions through the files agent.
pub struct AgentSharing {
    system: Arc<dyn AgentTransport>,
    files: Arc<dyn AgentTransport>,
}

impl AgentSharing {
    pub fn new(system: Arc<dyn AgentTransport>, files: Arc<dyn AgentTransport>) -> Self {
        Self { system, files }
    }
}

async fn change(
    transport: &dyn AgentTransport,
    request: Request,
    context: &RequestContext,
) -> Result<(), HostError> {
    match call_raw(transport, request, context).await? {
        Response::Done => Ok(()),
        Response::Error { code: ErrorCode::Forbidden, message } => Err(HostError::Refused(message)),
        Response::Error { code: ErrorCode::Conflict, .. } => Err(HostError::Conflict),
        Response::Error { code, message } => Err(agent_error(code, &message).into()),
        other => Err(unexpected(other).into()),
    }
}

fn invalid(what: &str) -> HostError {
    HostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn share(name: &ShareName) -> Result<proto::ShareName, HostError> {
    proto::ShareName::parse(name.as_str()).map_err(|_| invalid("share name"))
}

fn volume(number: u16) -> Result<proto::VolumeNumber, HostError> {
    proto::VolumeNumber::new(number).map_err(|_| invalid("volume number"))
}

fn pool(name: &str) -> Result<proto::PoolName, HostError> {
    proto::PoolName::parse(name).map_err(|_| invalid("pool name"))
}

fn principal(principal: &Principal) -> Result<proto::Principal, HostError> {
    Ok(match principal {
        Principal::User(name) => {
            proto::Principal::User(UserName::parse(name).map_err(|_| invalid("user name"))?)
        }
        Principal::Group(name) => {
            proto::Principal::Group(GroupName::parse(name).map_err(|_| invalid("group name"))?)
        }
    })
}

fn access(rules: &[(Principal, Permission)]) -> Result<Vec<ShareAccess>, HostError> {
    rules
        .iter()
        .map(|(p, permission)| {
            let permission = match permission {
                Permission::Deny => proto::SharePermission::Deny,
                Permission::Read => proto::SharePermission::Read,
                Permission::ReadWrite => proto::SharePermission::ReadWrite,
            };
            Ok(ShareAccess { principal: principal(p)?, permission })
        })
        .collect()
}

fn frequency(frequency: Frequency) -> SnapshotFrequency {
    match frequency {
        Frequency::Hourly => SnapshotFrequency::Hourly,
        Frequency::Daily => SnapshotFrequency::Daily,
        Frequency::Weekly => SnapshotFrequency::Weekly,
        Frequency::Monthly => SnapshotFrequency::Monthly,
    }
}

fn label(label: &str) -> Result<SnapshotLabel, HostError> {
    SnapshotLabel::parse(label).map_err(|_| invalid("snapshot label"))
}

/// `zfs list -H -p -t snapshot -o name,creation,used,referenced,
/// zendiloos:schedule,zendiloos:note`: the snapshots of share datasets
/// (`<pool>/<share>@zdl-…`); everything else ZFS has is left out.
pub fn parse_snapshots(text: &str) -> Vec<Snapshot> {
    let mut snapshots = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, creation, used, referenced, schedules, note] = fields[..] else { continue };
        let Some((dataset, label)) = name.split_once('@') else { continue };
        let Some((pool, share)) = dataset.split_once('/') else { continue };
        let (Ok(share), true) = (ShareName::parse(share), is_label(label)) else { continue };
        let (Ok(creation), Ok(used), Ok(referenced)) = (creation.parse(), used.parse(), referenced.parse())
        else {
            continue;
        };
        let schedules = if schedules == "-" {
            Vec::new()
        } else {
            schedules.split(',').filter_map(Frequency::parse).collect()
        };
        snapshots.push(Snapshot {
            pool: pool.to_owned(),
            share,
            label: label.to_owned(),
            taken_at: Timestamp::from_unix_seconds(creation),
            schedules,
            note: (note != "-" && !note.is_empty()).then(|| note.to_owned()),
            used_bytes: used,
            referenced_bytes: referenced,
        });
    }
    snapshots
}

/// `zfs list -H -p -t filesystem -d 1 -o name,used,available`: the
/// datasets directly below a pool, as shares; pools themselves are left out.
pub fn parse_usage(text: &str) -> Vec<ShareUsage> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (name, used, available) = (fields.next()?, fields.next()?, fields.next()?);
            let (pool, share) = name.split_once('/')?;
            Some(ShareUsage {
                pool: pool.to_owned(),
                name: ShareName::parse(share).ok()?,
                used_bytes: used.parse().ok()?,
                available_bytes: available.parse().ok()?,
            })
        })
        .collect()
}

#[async_trait]
impl ShareHost for AgentSharing {
    async fn set_properties(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        settings: &ShareSettings,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::SetShareProperties {
            pool: pool(pool_name)?,
            volume: volume(number)?,
            share: share(name)?,
            quota_bytes: settings.quota_bytes,
            compression: match settings.compression {
                Compression::Off => ShareCompression::Off,
                Compression::Lz4 => ShareCompression::Lz4,
                Compression::Zstd => ShareCompression::Zstd,
            },
            record_size: match settings.record_size {
                RecordSize::K16 => ShareRecordSize::K16,
                RecordSize::K128 => ShareRecordSize::K128,
                RecordSize::M1 => ShareRecordSize::M1,
            },
        };
        change(self.system.as_ref(), request, context).await
    }

    async fn usage(&self, context: &RequestContext) -> Result<Vec<ShareUsage>, HostError> {
        match call_raw(self.system.as_ref(), Request::ShareUsage {}, context).await? {
            Response::ShareUsage { zfs_list } => Ok(parse_usage(&zfs_list)),
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn snapshots(&self, context: &RequestContext) -> Result<Vec<Snapshot>, HostError> {
        match call_raw(self.system.as_ref(), Request::ListSnapshots {}, context).await? {
            Response::Snapshots { zfs_list } => Ok(parse_snapshots(&zfs_list)),
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn create_snapshot(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        snapshot: &str,
        schedules: &[Frequency],
        note: Option<&str>,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::CreateSnapshot {
            pool: pool(pool_name)?,
            volume: volume(number)?,
            share: share(name)?,
            label: label(snapshot)?,
            schedules: schedules.iter().copied().map(frequency).collect(),
            note: note.map(SnapshotNote::parse).transpose().map_err(|_| invalid("snapshot note"))?,
        };
        change(self.system.as_ref(), request, context).await
    }

    async fn destroy_snapshot(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        snapshot: &str,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::DestroySnapshot {
            pool: pool(pool_name)?,
            volume: volume(number)?,
            share: share(name)?,
            label: label(snapshot)?,
        };
        change(self.system.as_ref(), request, context).await
    }

    async fn rollback_snapshot(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        snapshot: &str,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::RollbackSnapshot {
            pool: pool(pool_name)?,
            volume: volume(number)?,
            share: share(name)?,
            label: label(snapshot)?,
        };
        change(self.system.as_ref(), request, context).await
    }

    async fn create_dataset(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request =
            Request::CreateShare { pool: pool(pool_name)?, volume: volume(number)?, name: share(name)? };
        change(self.system.as_ref(), request, context).await
    }

    async fn destroy_dataset(
        &self,
        pool_name: &str,
        number: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request =
            Request::DestroyShare { pool: pool(pool_name)?, volume: volume(number)?, name: share(name)? };
        change(self.system.as_ref(), request, context).await
    }

    async fn apply(&self, config: &SharingConfig, context: &RequestContext) -> Result<(), HostError> {
        let smb = config
            .smb
            .iter()
            .map(|spec| {
                Ok(SmbShare {
                    name: share(&spec.name)?,
                    volume: volume(spec.volume)?,
                    access: access(&spec.access)?,
                    recycle_bin: spec.recycle_bin,
                    // What Samba could misread is dropped from the text.
                    comment: spec.comment.as_deref().and_then(ShareComment::lossy),
                    time_machine: spec.time_machine,
                })
            })
            .collect::<Result<_, HostError>>()?;
        let nfs = config
            .nfs
            .iter()
            .map(|spec| {
                let clients = spec
                    .clients
                    .iter()
                    .map(|c| {
                        let network = proto::Network::parse(&c.network).map_err(|_| invalid("network"))?;
                        Ok(NfsClient { network, write: c.write })
                    })
                    .collect::<Result<_, HostError>>()?;
                Ok(NfsExport { name: share(&spec.name)?, volume: volume(spec.volume)?, clients })
            })
            .collect::<Result<_, HostError>>()?;
        let services = proto::FileServices {
            smb: config.services.smb,
            nfs: config.services.nfs,
            workgroup: proto::Workgroup::parse(&config.services.workgroup)
                .map_err(|_| invalid("workgroup"))?,
            smb_min_protocol: match config.services.smb_min_protocol {
                SmbProtocol::Smb2 => proto::SmbProtocol::Smb2,
                SmbProtocol::Smb2_1 => proto::SmbProtocol::Smb2_1,
                SmbProtocol::Smb3 => proto::SmbProtocol::Smb3,
            },
            smb_encryption: match config.services.smb_encryption {
                SmbEncryption::Default => proto::SmbEncryption::Default,
                SmbEncryption::Required => proto::SmbEncryption::Required,
                SmbEncryption::Off => proto::SmbEncryption::Off,
            },
            smb_signing: match config.services.smb_signing {
                SmbSigning::Default => proto::SmbSigning::Default,
                SmbSigning::Mandatory => proto::SmbSigning::Mandatory,
                SmbSigning::Disabled => proto::SmbSigning::Disabled,
            },
            hide_unreadable: config.services.hide_unreadable,
            multichannel: config.services.multichannel,
            oplocks: config.services.oplocks,
            leases: config.services.leases,
            durable_handles: config.services.durable_handles,
            mac_compat: config.services.mac_compat,
            follow_symlinks: config.services.follow_symlinks,
            veto_files: config
                .services
                .veto_files
                .as_deref()
                .map(|pattern| proto::VetoFiles::parse(pattern).map_err(|_| invalid("pattern of files")))
                .transpose()?,
            delete_veto_files: config.services.delete_veto_files,
            wsdd: config.services.wsdd,
            bonjour: config.services.bonjour,
        };
        let homes = match &config.homes {
            Some(homes) => Some(proto::HomesShare {
                volume: volume(homes.volume)?,
                users: homes
                    .users
                    .iter()
                    .map(|name| UserName::parse(name).map_err(|_| invalid("user name")))
                    .collect::<Result<_, _>>()?,
            }),
            None => None,
        };
        change(self.system.as_ref(), Request::ApplySharing { smb, nfs, services, homes }, context).await
    }

    async fn prepare_homes(
        &self,
        pool_name: &str,
        number: u16,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::PrepareHomes { pool: pool(pool_name)?, volume: volume(number)? };
        change(self.system.as_ref(), request, context).await
    }

    async fn set_home(
        &self,
        pool_name: &str,
        number: u16,
        user: &str,
        quota_bytes: Option<u64>,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let user = UserName::parse(user).map_err(|_| invalid("user name"))?;
        // The folder through the files agent, the only one that writes to
        // the volumes; the quota through the system agent, which runs zfs.
        let make = Request::MakeHome { volume: volume(number)?, user: user.clone() };
        change(self.files.as_ref(), make, context).await?;
        let quota =
            Request::SetHomeQuota { pool: pool(pool_name)?, volume: volume(number)?, user, quota_bytes };
        change(self.system.as_ref(), quota, context).await
    }

    async fn set_access(
        &self,
        number: u16,
        name: &ShareName,
        rules: &[(Principal, Permission)],
        removed: &[Principal],
        recursive: bool,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::SetShareAccess {
            volume: volume(number)?,
            name: share(name)?,
            access: access(rules)?,
            removed: removed.iter().map(principal).collect::<Result<_, _>>()?,
            recursive,
        };
        change(self.files.as_ref(), request, context).await
    }
}

/// Volumes for shares: the pools zendiloos created.
#[async_trait]
impl Volumes for AgentStorage {
    async fn volumes(&self, context: &RequestContext) -> Result<Vec<Volume>, PortError> {
        Ok(self
            .pools(context)
            .await?
            .into_iter()
            .filter_map(|pool| {
                let number = pool.volume?.get();
                Some(Volume { pool: pool.name, number, mounted: pool.space.is_some_and(|s| s.mounted) })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The context names snapshots, the agent tells Samba how to read the
    /// names: both must agree, or Windows shows no "Previous Versions".
    #[test]
    fn snapshot_names_are_what_the_agent_and_samba_expect() {
        let label = zendiloos_sharing::domain::label_at(Timestamp::from_unix_seconds(1_793_907_601));
        assert!(SnapshotLabel::parse(&label).is_ok(), "{label}");
        assert_eq!(zendiloos_sharing::domain::LABEL_FORMAT, proto::SNAPSHOT_LABEL_FORMAT);
    }

    #[test]
    fn reads_what_the_shares_hold() {
        let text = "tank\t3221225472\t5000000000\ntank/photos\t1073741824\t4294967296\nbackup/old\t5\t6\nrpool/ROOT\t1\t2\n";
        let usage = parse_usage(text);
        assert_eq!(usage.len(), 2, "the pool itself and names no share can have are left out");
        assert_eq!(
            (usage[0].pool.as_str(), usage[0].name.as_str(), usage[0].used_bytes),
            ("tank", "photos", 1 << 30)
        );
        assert_eq!(usage[0].available_bytes, 4 << 30);
    }

    #[test]
    fn reads_the_snapshots_of_shares_only() {
        let text = "tank/photos@zdl-2026-11-05_19.40.01\t1793907601\t4096\t1048576\thourly,daily\t-\n\
                    tank/photos@zdl-2026-11-05_20.00.00\t1793908800\t0\t1048576\t-\tbefore the move\n\
                    tank@zdl-2026-11-05_20.00.00\t1793908800\t0\t1\t-\t-\n\
                    rpool/ROOT/debian@install\t1700000000\t0\t1\t-\t-\n\
                    tank/photos@manual-by-hand\t1793908800\t0\t1\t-\t-\n";

        let snapshots = parse_snapshots(text);

        assert_eq!(snapshots.len(), 2);
        assert_eq!((snapshots[0].pool.as_str(), snapshots[0].share.as_str()), ("tank", "photos"));
        assert_eq!(snapshots[0].schedules, [Frequency::Hourly, Frequency::Daily]);
        assert_eq!((snapshots[0].used_bytes, snapshots[0].referenced_bytes), (4096, 1_048_576));
        assert_eq!(snapshots[0].taken_at, Timestamp::from_unix_seconds(1_793_907_601));
        assert_eq!(snapshots[1].note.as_deref(), Some("before the move"));
        assert!(snapshots[1].schedules.is_empty());
    }
}
