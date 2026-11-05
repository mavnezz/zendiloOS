use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ErrorCode, GroupName, NfsClient, NfsExport, Request, Response,
    ShareAccess, ShareComment, SmbShare, SnapshotFrequency, SnapshotLabel, SnapshotNote, UserName,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_sharing::application::ports::{HostError, ShareHost, Volume, Volumes};
use zendiloos_sharing::domain::{Access, Frequency, Principal, ShareName, SharingConfig, Snapshot, is_label};
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

fn access(rules: &[(Principal, Access)]) -> Result<Vec<ShareAccess>, HostError> {
    rules
        .iter()
        .map(|(p, a)| Ok(ShareAccess { principal: principal(p)?, write: *a == Access::ReadWrite }))
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

#[async_trait]
impl ShareHost for AgentSharing {
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
        change(self.system.as_ref(), Request::ApplySharing { smb, nfs }, context).await
    }

    async fn set_access(
        &self,
        number: u16,
        name: &ShareName,
        rules: &[(Principal, Access)],
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
