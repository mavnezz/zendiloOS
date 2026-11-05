//! A simulated agent behind the same [`AgentTransport`] as the real one.
//!
//! zendiloosd uses it on a development machine: everything above the
//! transport, including the parsing of tool output, runs as in production,
//! but nothing is executed and nothing needs root.
//!
//! The simulated host is a small NAS (`data/`): a system NVMe, a healthy
//! and a degraded RAID 1 with a Btrfs volume each, a failing disk, a
//! sleeping disk and two unused ones. Its users and groups are those of a
//! Debian system plus whatever is created while it runs; any other name
//! counts as managed, so accounts from an earlier run keep working.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, DiskName, ErrorCode, PROTOCOL_VERSION, Request, RequestId, Response, TransportError,
};

/// Users and groups of the simulated Debian system, which the agent never
/// changes.
const SYSTEM_USERS: [&str; 7] = ["root", "daemon", "bin", "sys", "nobody", "debian", "zendiloos"];
const SYSTEM_GROUPS: [&str; 6] = ["root", "adm", "sudo", "users", "zendiloos", "zendiloos-users"];

const LSBLK: &str = include_str!("../data/lsblk.json");
const MDSTAT: &str = include_str!("../data/mdstat.txt");
const SMART_HDD: &str = include_str!("../data/smartctl-hdd.json");
const SMART_HDD_FAILING: &str = include_str!("../data/smartctl-hdd-failing.json");
const SMART_NVME: &str = include_str!("../data/smartctl-nvme.json");
const SMART_STANDBY: &str = include_str!("../data/smartctl-standby.json");
const SMART_UNKNOWN: &str = r#"{"smartctl": {"messages": [{"string": "/dev/{{DEVICE}}: No such device",
    "severity": "error"}], "exit_status": 2}}"#;

#[derive(Debug, Default)]
pub struct SimulatedAgent {
    received: Mutex<Vec<Request>>,
    /// Users and groups created while running.
    created: Mutex<HashSet<(Kind, String)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    User,
    Group,
}

fn system(kind: Kind, name: &str) -> bool {
    match kind {
        Kind::User => SYSTEM_USERS.contains(&name),
        Kind::Group => SYSTEM_GROUPS.contains(&name),
    }
}

fn error(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

impl SimulatedAgent {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every request received so far, oldest first.
    pub fn received(&self) -> Vec<Request> {
        self.received.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn create(&self, kind: Kind, name: &str) -> Response {
        let mut created = self.created.lock().unwrap_or_else(PoisonError::into_inner);
        if system(kind, name) || !created.insert((kind, name.to_owned())) {
            return error(ErrorCode::Conflict, "the name is already taken");
        }
        Response::Done
    }

    fn change(&self, kind: Kind, name: &str, delete: bool) -> Response {
        if system(kind, name) {
            return error(ErrorCode::Forbidden, "not managed by zendiloos");
        }
        if delete {
            self.created.lock().unwrap_or_else(PoisonError::into_inner).remove(&(kind, name.to_owned()));
        }
        Response::Done
    }
}

#[async_trait]
impl AgentTransport for SimulatedAgent {
    async fn call(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<Response, TransportError> {
        let request_id = request_id.as_ref().map(RequestId::as_str).unwrap_or("-");
        let response = match &request {
            Request::Hello {} => Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                agent_version: concat!(env!("CARGO_PKG_VERSION"), "-simulated").to_owned(),
            },
            Request::Reboot {} | Request::PowerOff {} => {
                tracing::warn!(op = request.name(), request_id, "simulated agent: not executing");
                Response::Done
            }
            Request::ListBlockDevices {} => Response::BlockDevices { lsblk_json: LSBLK.to_owned() },
            Request::SmartData { device } => smart(device),
            Request::ArrayStatus {} => Response::ArrayStatus { mdstat: MDSTAT.to_owned() },
            Request::CreateUser { name } => self.create(Kind::User, name.as_str()),
            Request::DeleteUser { name } => self.change(Kind::User, name.as_str(), true),
            Request::SetSmbPassword { name, .. } | Request::SetUserEnabled { name, .. } => {
                self.change(Kind::User, name.as_str(), false)
            }
            Request::CreateGroup { name } => self.create(Kind::Group, name.as_str()),
            Request::DeleteGroup { name } => self.change(Kind::Group, name.as_str(), true),
            Request::SetGroupMembers { group, members } => {
                match members.iter().find(|member| system(Kind::User, member.as_str())) {
                    Some(_) => error(ErrorCode::Forbidden, "not managed by zendiloos"),
                    None => self.change(Kind::Group, group.as_str(), false),
                }
            }
        };
        self.received.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        Ok(response)
    }
}

/// smartctl's answer for each simulated disk, with its exit status.
fn smart(device: &DiskName) -> Response {
    let (template, exit_status, model, serial) = match device.as_str() {
        "sda" => (SMART_HDD, 0, "", "SIM4T0001"),
        "sdb" => (SMART_HDD, 0, "", "SIM4T0002"),
        "sdc" => (SMART_HDD, 0, "", "SIM4T0003"),
        "sdd" => (SMART_HDD_FAILING, 8 | 16 | 64 | 128, "", "SIM4T0004"),
        "sde" => (SMART_STANDBY, 2, "", ""),
        "nvme0n1" => (SMART_NVME, 0, "SIM NVMe SSD 512GB", "SIMNV0001"),
        "nvme1n1" => (SMART_NVME, 0, "SIM NVMe SSD 1TB", "SIMNV0002"),
        _ => (SMART_UNKNOWN, 2, "", ""),
    };
    let smartctl_json = template
        .replace("{{DEVICE}}", device.as_str())
        .replace("{{MODEL}}", model)
        .replace("{{SERIAL}}", serial);
    Response::Smart { exit_status, smartctl_json }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_like_the_agent_without_doing_anything() {
        let agent = SimulatedAgent::new();
        let hello = agent.call(Request::Hello {}, None).await.unwrap();
        assert!(matches!(hello, Response::Hello { protocol_version: PROTOCOL_VERSION, .. }));
        assert_eq!(agent.call(Request::Reboot {}, None).await.unwrap(), Response::Done);
        assert_eq!(agent.received(), [Request::Hello {}, Request::Reboot {}]);
    }
}
