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

use super::{WHAT, agent_error, call_raw, call_raw_reporting, refusal, unexpected};
use crate::agent::AgentStorage;

/// [`ShareHost`] through the agents: datasets and server configuration
/// through the system agent, the shares' folders through the files agent.
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
        Response::Error { code: ErrorCode::Forbidden, message } => Err(HostError::Refused(refusal(&message))),
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

/// The lines the files agent reports as a walk goes, to `lines`; its steps
/// are not used.
struct Lines<'a>(&'a (dyn for<'line> Fn(&'line str) + Send + Sync));

impl proto::Report for Lines<'_> {
    fn step(&self, _: &str, _: Option<u8>) {}

    fn line(&self, line: &str) {
        (self.0)(line);
    }
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

    async fn apply(
        &self,
        config: &SharingConfig,
        close: &[ShareName],
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let smb = config
            .smb
            .iter()
            .map(|spec| {
                Ok(SmbShare {
                    name: share(&spec.name)?,
                    volume: volume(spec.volume)?,
                    access: access(&spec.access)?,
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
                        let network = proto::NfsHost::parse(&c.network).map_err(|_| invalid("network"))?;
                        Ok(NfsClient {
                            network,
                            write: c.write,
                            asynchronous: c.asynchronous,
                            insecure: c.insecure,
                        })
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
        let close = close.iter().map(share).collect::<Result<_, _>>()?;
        change(self.system.as_ref(), Request::ApplySharing { smb, nfs, services, homes, close }, context)
            .await
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

    async fn prepare_share(
        &self,
        number: u16,
        name: &ShareName,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::PrepareShare { volume: volume(number)?, name: share(name)? };
        change(self.files.as_ref(), request, context).await
    }

    async fn migrate_share(
        &self,
        number: u16,
        name: &ShareName,
        lines: &(dyn for<'line> Fn(&'line str) + Send + Sync),
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let request = Request::MigrateShare { volume: volume(number)?, name: share(name)? };
        let report = Lines(lines);
        match call_raw_reporting(self.files.as_ref(), request, &report, context).await? {
            Response::Done => Ok(()),
            Response::Error { code: ErrorCode::Forbidden, message } => {
                Err(HostError::Refused(refusal(&message)))
            }
            Response::Error { code, message } => Err(agent_error(code, &message).into()),
            other => Err(unexpected(other).into()),
        }
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
    use zendiloos_sharing::domain::{FileServices, HomesSpec, NfsClient as Client, NfsSpec, SmbSpec};

    use super::*;
    use crate::agent::scripted::Scripted;

    fn photos() -> ShareName {
        ShareName::parse("photos").unwrap()
    }

    fn host(system: &Arc<Scripted>, files: &Arc<Scripted>) -> AgentSharing {
        AgentSharing::new(system.clone(), files.clone())
    }

    fn agents() -> (Arc<Scripted>, Arc<Scripted>) {
        (Arc::new(Scripted::default()), Arc::new(Scripted::default()))
    }

    #[tokio::test]
    async fn the_agents_refusals_and_failures_are_told_apart() {
        let context = RequestContext::default();
        let refused = Arc::new(Scripted::refusing(ErrorCode::Forbidden, "not managed"));
        let error = host(&refused, &refused).create_dataset("tank", 1, &photos(), &context).await;
        assert!(matches!(error, Err(HostError::Refused(ref why)) if why == "not managed"), "{error:?}");
        let taken = Arc::new(Scripted::refusing(ErrorCode::Conflict, "exists"));
        let error = host(&taken, &taken).create_dataset("tank", 1, &photos(), &context).await;
        assert!(matches!(error, Err(HostError::Conflict)), "{error:?}");
        let failed = Arc::new(Scripted::refusing(ErrorCode::Failed, "zfs failed"));
        let error = host(&failed, &failed).destroy_dataset("tank", 1, &photos(), &context).await;
        assert!(matches!(error, Err(HostError::Port(_))), "{error:?}");
        let odd = Arc::new(Scripted::answering([Response::ShareUsage { zfs_list: String::new() }]));
        let error = host(&odd, &odd).destroy_dataset("tank", 1, &photos(), &context).await;
        assert!(matches!(error, Err(HostError::Port(_))), "{error:?}");
        let error = host(&odd, &odd).create_dataset("not a pool!", 1, &photos(), &context).await;
        assert!(matches!(error, Err(HostError::Port(_))), "an odd name is not sent: {error:?}");
        let error = host(&odd, &odd).create_dataset("tank", 0, &photos(), &context).await;
        assert!(matches!(error, Err(HostError::Port(_))), "nor volume 0: {error:?}");
    }

    #[tokio::test]
    async fn datasets_properties_and_snapshots_become_requests() {
        let (system, files) = agents();
        let sharing = host(&system, &files);
        let context = RequestContext::default();
        let label = zendiloos_sharing::domain::label_at(Timestamp::from_unix_seconds(1_793_907_601));
        for (compression, record_size) in [
            (Compression::Off, RecordSize::K16),
            (Compression::Lz4, RecordSize::K128),
            (Compression::Zstd, RecordSize::M1),
        ] {
            let settings = ShareSettings { quota_bytes: Some(1 << 30), compression, record_size };
            sharing.set_properties("tank", 1, &photos(), &settings, &context).await.unwrap();
        }
        sharing.create_dataset("tank", 1, &photos(), &context).await.unwrap();
        sharing.destroy_dataset("tank", 1, &photos(), &context).await.unwrap();
        let schedules = [Frequency::Hourly, Frequency::Daily, Frequency::Weekly, Frequency::Monthly];
        sharing
            .create_snapshot("tank", 1, &photos(), &label, &schedules, Some("before"), &context)
            .await
            .unwrap();
        sharing.destroy_snapshot("tank", 1, &photos(), &label, &context).await.unwrap();
        sharing.rollback_snapshot("tank", 1, &photos(), &label, &context).await.unwrap();
        sharing.prepare_homes("tank", 1, &context).await.unwrap();

        let asked = system.asked();
        assert_eq!(asked.len(), 9, "{asked:?}");
        assert!(matches!(
            &asked[2],
            Request::SetShareProperties { compression: ShareCompression::Zstd, record_size: ShareRecordSize::M1, quota_bytes: Some(q), .. }
                if *q == 1 << 30
        ));
        assert!(matches!(&asked[3], Request::CreateShare { name, .. } if name.as_str() == "photos"));
        assert!(matches!(&asked[4], Request::DestroyShare { .. }));
        assert!(
            matches!(&asked[5], Request::CreateSnapshot { schedules, note: Some(_), .. } if schedules.len() == 4)
        );
        assert!(matches!(&asked[6], Request::DestroySnapshot { .. }));
        assert!(matches!(&asked[7], Request::RollbackSnapshot { .. }));
        assert!(matches!(&asked[8], Request::PrepareHomes { .. }));
        assert!(files.asked().is_empty(), "the system agent's work");
        let odd = sharing.create_snapshot("tank", 1, &photos(), "not-a-label", &[], None, &context).await;
        assert!(matches!(odd, Err(HostError::Port(_))));
    }

    #[tokio::test]
    async fn usage_and_snapshots_are_read_from_the_agents_answer() {
        let context = RequestContext::default();
        let system = Arc::new(Scripted::answering([
            Response::ShareUsage { zfs_list: "tank/photos\t1\t2\n".into() },
            Response::Snapshots {
                zfs_list: "tank/photos@zdl-2026-11-05_19.40.01\t1793907601\t1\t2\t-\t-\n".into(),
            },
            Response::Error { code: ErrorCode::Failed, message: "zfs".into() },
            Response::Done,
        ]));
        let sharing = host(&system, &system);
        assert_eq!(sharing.usage(&context).await.unwrap()[0].used_bytes, 1);
        assert_eq!(sharing.snapshots(&context).await.unwrap()[0].share.as_str(), "photos");
        assert!(sharing.usage(&context).await.is_err());
        assert!(sharing.snapshots(&context).await.is_err(), "another answer");
    }

    #[tokio::test]
    async fn the_servers_configuration_is_sent_as_typed_values() {
        let (system, files) = agents();
        let sharing = host(&system, &files);
        let context = RequestContext::default();
        let config = |services: FileServices| SharingConfig {
            smb: vec![SmbSpec {
                name: photos(),
                volume: 1,
                access: vec![
                    (Principal::User("anna".into()), Permission::ReadWrite),
                    (Principal::Group("family".into()), Permission::Read),
                    (Principal::User("ben".into()), Permission::Deny),
                ],
                comment: Some("Holidays\nand more".into()),
                time_machine: Some(None),
            }],
            nfs: vec![NfsSpec {
                name: photos(),
                volume: 1,
                clients: vec![Client::new("192.168.1.0/24", true)],
            }],
            services,
            homes: Some(HomesSpec { volume: 1, users: vec!["anna".into()] }),
        };
        let all = [
            (SmbProtocol::Smb2, SmbEncryption::Default, SmbSigning::Default),
            (SmbProtocol::Smb2_1, SmbEncryption::Required, SmbSigning::Mandatory),
            (SmbProtocol::Smb3, SmbEncryption::Off, SmbSigning::Disabled),
        ];
        for (smb_min_protocol, smb_encryption, smb_signing) in all {
            let services = FileServices {
                smb_min_protocol,
                smb_encryption,
                smb_signing,
                veto_files: Some("/*.tmp/".into()),
                ..FileServices::default()
            };
            sharing.apply(&config(services), &[photos()], &context).await.unwrap();
        }
        let Request::ApplySharing { smb, nfs, services, homes, close } = &system.asked()[2] else { panic!() };
        assert_eq!(close.iter().map(proto::ShareName::as_str).collect::<Vec<_>>(), ["photos"]);
        assert_eq!(smb[0].access.len(), 3);
        assert_eq!(nfs[0].clients.len(), 1);
        assert_eq!(services.smb_min_protocol, proto::SmbProtocol::Smb3);
        assert_eq!(homes.as_ref().unwrap().users.len(), 1);

        let odd_network = SharingConfig {
            nfs: vec![NfsSpec { name: photos(), volume: 1, clients: vec![Client::new("x y", false)] }],
            ..SharingConfig::default()
        };
        assert!(matches!(sharing.apply(&odd_network, &[], &context).await, Err(HostError::Port(_))));
        let odd_user = SharingConfig {
            smb: vec![SmbSpec {
                name: photos(),
                volume: 1,
                access: vec![(Principal::User("not a name".into()), Permission::Read)],
                comment: None,
                time_machine: None,
            }],
            ..SharingConfig::default()
        };
        assert!(matches!(sharing.apply(&odd_user, &[], &context).await, Err(HostError::Port(_))));
        let odd_group = SharingConfig {
            smb: vec![SmbSpec {
                name: photos(),
                volume: 1,
                access: vec![(Principal::Group("@x y".into()), Permission::Read)],
                comment: None,
                time_machine: None,
            }],
            ..SharingConfig::default()
        };
        assert!(matches!(sharing.apply(&odd_group, &[], &context).await, Err(HostError::Port(_))));
        let odd_veto = SharingConfig {
            services: FileServices { veto_files: Some("*.tmp".into()), ..FileServices::default() },
            ..SharingConfig::default()
        };
        assert!(matches!(sharing.apply(&odd_veto, &[], &context).await, Err(HostError::Port(_))));
        let odd_home = SharingConfig {
            homes: Some(HomesSpec { volume: 1, users: vec!["not a name".into()] }),
            ..SharingConfig::default()
        };
        assert!(matches!(sharing.apply(&odd_home, &[], &context).await, Err(HostError::Port(_))));
        assert_eq!(system.asked().len(), 3, "nothing odd was sent");
    }

    #[tokio::test]
    async fn folders_go_to_the_files_agent_quotas_to_the_system_agent() {
        let (system, files) = agents();
        let sharing = host(&system, &files);
        let context = RequestContext::default();
        sharing.set_home("tank", 1, "anna", Some(1 << 30), &context).await.unwrap();
        sharing.prepare_share(1, &photos(), &context).await.unwrap();
        let lines = std::sync::Mutex::new(Vec::new());
        let note = |line: &str| lines.lock().unwrap().push(line.to_owned());
        sharing.migrate_share(1, &photos(), &note, &context).await.unwrap();
        assert!(matches!(&system.only(), Request::SetHomeQuota { quota_bytes: Some(_), .. }));
        let asked = files.asked();
        assert!(matches!(&asked[0], Request::MakeHome { user, .. } if user.as_str() == "anna"));
        assert!(matches!(&asked[1], Request::PrepareShare { name, .. } if name.as_str() == "photos"));
        assert!(matches!(&asked[2], Request::MigrateShare { name, .. } if name.as_str() == "photos"));
        assert!(sharing.set_home("tank", 1, "not a name", None, &context).await.is_err());
    }

    #[tokio::test]
    async fn a_walk_tells_how_far_it_got_and_a_refusal_is_one() {
        let context = RequestContext::default();
        let files = Arc::new(Scripted::reporting(
            vec![Response::Output { line: "10000 entries".into() }],
            Response::Done,
        ));
        let sharing = host(&Arc::new(Scripted::default()), &files);
        let lines = std::sync::Mutex::new(Vec::new());
        let note = |line: &str| lines.lock().unwrap().push(line.to_owned());
        sharing.migrate_share(1, &photos(), &note, &context).await.unwrap();
        assert_eq!(*lines.lock().unwrap(), ["10000 entries"]);

        let busy = Arc::new(Scripted::refusing(ErrorCode::Forbidden, "share photos is not mounted"));
        let refused = host(&busy, &busy).migrate_share(1, &photos(), &note, &context).await;
        assert!(
            matches!(refused, Err(HostError::Refused(ref why)) if why.contains("not mounted")),
            "{refused:?}"
        );
        let failed = Arc::new(Scripted::refusing(ErrorCode::Failed, "3 entries could not be changed"));
        let failed = host(&failed, &failed).migrate_share(1, &photos(), &note, &context).await;
        assert!(matches!(failed, Err(HostError::Port(_))), "{failed:?}");
    }

    #[tokio::test]
    async fn the_volumes_for_shares_are_the_pools_zendiloos_made() {
        use zendiloos_storage::application::ports::ZfsSource;
        let agent = zendiloos_sim::SimulatedAgent::new();
        let storage = AgentStorage::new(Arc::new(agent));
        let pools = storage.pools(&RequestContext::default()).await.unwrap();
        let volumes = Volumes::volumes(&storage, &RequestContext::default()).await.unwrap();
        let managed = pools.iter().filter(|pool| pool.volume.is_some()).count();
        assert_eq!(volumes.len(), managed);
    }

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
