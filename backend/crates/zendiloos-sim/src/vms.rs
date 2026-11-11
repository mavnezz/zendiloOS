//! Virtual machines, simulated: libvirt's domains as a map, their states
//! as `virsh list --all` prints them, and a console that speaks just
//! enough of VNC (RFB 3.8, no authentication, raw pixels) for noVNC to show
//! a picture. Nothing runs.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use zendiloos_agent_proto::{ErrorCode, Request, Response, VmAction, VmId, VmNetwork, VmStorage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Power {
    Off,
    Running,
}

#[derive(Debug, Default)]
pub(crate) struct SimVms {
    /// libvirt and QEMU installed (the plugin's run of apt).
    pub(crate) installed: bool,
    storage: Option<VmStorage>,
    domains: BTreeMap<VmId, Power>,
    /// Domain ids since libvirt started, as `virsh list` numbers them.
    started: u32,
}

fn error(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

impl SimVms {
    /// The answer to a request about VMs; `None` for other requests.
    pub(crate) fn handle(&mut self, request: &Request) -> Option<Response> {
        let not_installed = || error(ErrorCode::Forbidden, "refused: libvirt and QEMU are not installed");
        Some(match request {
            Request::VmHost {} => Response::VmHost {
                kvm: true,
                cpu_virtualization: Some("vmx".into()),
                iommu: false,
                installed: self.installed,
                cores: 8,
                memory_bytes: 16 << 30,
                storage: self.storage.clone(),
            },
            Request::EnableVms { .. } if !self.installed => not_installed(),
            Request::EnableVms { pool, volume } => match &self.storage {
                Some(storage) if storage.volume != *volume => error(
                    ErrorCode::Forbidden,
                    "refused: the virtual machines' storage is on another pool already",
                ),
                _ => {
                    self.storage = Some(VmStorage { pool: pool.clone(), volume: *volume });
                    Response::Done
                }
            },
            Request::CopyVmIso { volume, .. } | Request::DeleteVm { volume, .. }
                if self.storage.as_ref().is_none_or(|storage| storage.volume != *volume) =>
            {
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up there")
            }
            Request::CopyVmIso { .. } => Response::Done,
            Request::DefineVm { .. } if !self.installed => not_installed(),
            Request::DefineVm { spec } => {
                if self.domains.contains_key(&spec.id) {
                    error(ErrorCode::Conflict, "the name is already taken")
                } else if let VmNetwork::Bridge { bridge } = &spec.network
                    && bridge.as_str() != "br0"
                {
                    error(
                        ErrorCode::Forbidden,
                        &format!("refused: {} is no bridge of this machine", bridge.as_str()),
                    )
                } else {
                    self.domains.insert(spec.id.clone(), Power::Off);
                    Response::Done
                }
            }
            Request::SetVmAutostart { id, .. } => {
                if self.domains.contains_key(id) {
                    Response::Done
                } else {
                    missing(id)
                }
            }
            Request::ControlVm { id, action } => {
                let Some(power) = self.domains.get_mut(id) else { return Some(missing(id)) };
                match (action, *power) {
                    (VmAction::Start, Power::Running) => error(
                        ErrorCode::Forbidden,
                        "refused: Requested operation is not valid: domain is already active",
                    ),
                    (VmAction::Start, Power::Off) => {
                        *power = Power::Running;
                        self.started += 1;
                        Response::Done
                    }
                    (_, Power::Off) => error(
                        ErrorCode::Forbidden,
                        "refused: Requested operation is not valid: domain is not running",
                    ),
                    // A guest asked to shut down does at once here.
                    (VmAction::Shutdown | VmAction::ForceOff, Power::Running) => {
                        *power = Power::Off;
                        Response::Done
                    }
                    (VmAction::Reboot, Power::Running) => Response::Done,
                }
            }
            Request::DeleteVm { id, .. } => {
                self.domains.remove(id);
                Response::Done
            }
            Request::VmStates {} => {
                let mut virsh_list = String::from(" Id   Name                     State\n");
                virsh_list.push_str("-------------------------------------------\n");
                for (number, (id, power)) in self.domains.iter().enumerate() {
                    let (shown, state) = match power {
                        Power::Running => ((number + 1).to_string(), "running"),
                        Power::Off => ("-".to_owned(), "shut off"),
                    };
                    virsh_list.push_str(&format!(" {shown:<4} {:<24} {state}\n", id.domain()));
                }
                // Someone else's domain, which zendiloOS never shows.
                virsh_list.push_str(" -    win10                    shut off\n");
                Response::VmStates { virsh_list }
            }
            Request::OpenVmConsole { .. } => {
                error(ErrorCode::InvalidRequest, "a console is opened with its descriptor")
            }
            _ => return None,
        })
    }

    /// The console of a running VM, as the agent answers: one side of a
    /// socket pair, whose other side a thread serves as a VNC server.
    pub(crate) fn console(&self, id: &VmId) -> (Response, Option<OwnedFd>) {
        if self.domains.get(id) != Some(&Power::Running) {
            let refused = "refused: the virtual machine is not running, or has no console";
            return (error(ErrorCode::Forbidden, refused), None);
        }
        let (ours, theirs) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(e) => return (error(ErrorCode::Failed, &format!("no console: {e}")), None),
        };
        let name = id.domain();
        std::thread::spawn(move || {
            let _ = serve_vnc(theirs, &name);
        });
        (Response::VmConsole {}, Some(OwnedFd::from(ours)))
    }
}

fn missing(id: &VmId) -> Response {
    error(ErrorCode::Forbidden, &format!("refused: there is no virtual machine {}", id.as_str()))
}

const WIDTH: u16 = 640;
const HEIGHT: u16 = 400;

/// How the client wants its pixels: 32 bits, the shifts of red, green and
/// blue, and their byte order.
#[derive(Clone, Copy)]
struct Format {
    big_endian: bool,
    shifts: [u8; 3],
}

/// RFB 3.8 without authentication; a gradient for the picture, sent whole
/// when asked for and never again, as nothing changes.
fn serve_vnc(mut stream: UnixStream, name: &str) -> std::io::Result<()> {
    let mut version = [0u8; 12];
    stream.write_all(b"RFB 003.008\n")?;
    stream.read_exact(&mut version)?;
    stream.write_all(&[1, 1])?; // one type of security: none
    let mut chosen = [0u8; 1];
    stream.read_exact(&mut chosen)?;
    stream.write_all(&0u32.to_be_bytes())?;
    let mut shared = [0u8; 1];
    stream.read_exact(&mut shared)?;
    let mut init = Vec::new();
    init.extend(WIDTH.to_be_bytes());
    init.extend(HEIGHT.to_be_bytes());
    // 32 bits per pixel, depth 24, little endian, true colour, 255 each,
    // red at 16, green at 8, blue at 0.
    init.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    let title = format!("{name} (simulated)");
    init.extend(u32::try_from(title.len()).unwrap_or(0).to_be_bytes());
    init.extend(title.as_bytes());
    stream.write_all(&init)?;
    let mut format = Format { big_endian: false, shifts: [16, 8, 0] };
    loop {
        let mut kind = [0u8; 1];
        stream.read_exact(&mut kind)?;
        match kind[0] {
            0 => {
                let mut rest = [0u8; 19];
                stream.read_exact(&mut rest)?;
                format = Format { big_endian: rest[5] != 0, shifts: [rest[13], rest[14], rest[15]] };
            }
            2 => {
                let mut head = [0u8; 3];
                stream.read_exact(&mut head)?;
                let count = u16::from_be_bytes([head[1], head[2]]);
                let mut encodings = vec![0u8; usize::from(count) * 4];
                stream.read_exact(&mut encodings)?;
            }
            3 => {
                let mut request = [0u8; 9];
                stream.read_exact(&mut request)?;
                if request[0] == 0 {
                    stream.write_all(&picture(format))?;
                }
            }
            4 => stream.read_exact(&mut [0u8; 7])?,
            5 => stream.read_exact(&mut [0u8; 5])?,
            6 => {
                let mut head = [0u8; 7];
                stream.read_exact(&mut head)?;
                let length = u32::from_be_bytes([head[3], head[4], head[5], head[6]]);
                std::io::copy(&mut (&mut stream).take(u64::from(length)), &mut std::io::sink())?;
            }
            _ => return Ok(()),
        }
    }
}

/// One update with the whole screen in raw pixels.
fn picture(format: Format) -> Vec<u8> {
    let (width, height) = (usize::from(WIDTH), usize::from(HEIGHT));
    let mut update = Vec::with_capacity(16 + width * height * 4);
    update.extend([0, 0]);
    update.extend(1u16.to_be_bytes());
    update.extend([0, 0, 0, 0]);
    update.extend(WIDTH.to_be_bytes());
    update.extend(HEIGHT.to_be_bytes());
    update.extend(0i32.to_be_bytes()); // raw
    for y in 0..height {
        for x in 0..width {
            let (red, green, blue) = ((x * 255 / width) as u32, (y * 255 / height) as u32, 160u32);
            let [r, g, b] = format.shifts.map(u32::from);
            let pixel = (red << r) | (green << g) | (blue << b);
            update.extend(if format.big_endian { pixel.to_be_bytes() } else { pixel.to_le_bytes() });
        }
    }
    update
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_speaks_rfb_without_authentication() {
        let mut vms = SimVms { installed: true, ..SimVms::default() };
        let id = VmId::parse("0a1b2c3d4e5f").unwrap();
        vms.domains.insert(id.clone(), Power::Off);
        assert!(vms.console(&id).1.is_none(), "not while it is off");
        vms.domains.insert(id.clone(), Power::Running);
        let mut console = UnixStream::from(vms.console(&id).1.unwrap());
        let mut version = [0u8; 12];
        console.read_exact(&mut version).unwrap();
        assert_eq!(&version, b"RFB 003.008\n");
        console.write_all(b"RFB 003.008\n").unwrap();
        let mut security = [0u8; 2];
        console.read_exact(&mut security).unwrap();
        assert_eq!(security, [1, 1]);
        console.write_all(&[1]).unwrap();
        let mut result = [0u8; 4];
        console.read_exact(&mut result).unwrap();
        assert_eq!(result, [0; 4]);
        console.write_all(&[1]).unwrap();
        let mut size = [0u8; 4];
        console.read_exact(&mut size).unwrap();
        assert_eq!(
            (u16::from_be_bytes([size[0], size[1]]), u16::from_be_bytes([size[2], size[3]])),
            (640, 400)
        );
    }
}
