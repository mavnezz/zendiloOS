//! Virtual machines, simulated: libvirt's domains as a map, their states
//! as `virsh list --all` prints them, and a console that speaks just
//! enough of VNC (RFB 3.8, no authentication, raw pixels) for noVNC to show
//! a picture. Nothing runs.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use zendiloos_agent_proto::{
    ErrorCode, Request, Response, VM_IMAGE_UPLOAD_MAX, VMS_DATASET, VmAction, VmDiskSlot, VmId, VmImageFile,
    VmImageId, VmNetwork, VmSpec, VmStorage, VolumeNumber,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Power {
    Off,
    Running,
}

/// An upload being read: the bytes that came once the pipe closed.
pub(crate) type Receiving = std::thread::JoinHandle<u64>;

#[derive(Debug, Default)]
pub(crate) struct SimVms {
    /// libvirt and QEMU installed (the plugin's run of apt).
    pub(crate) installed: bool,
    /// The locations of the VMs' storage.
    locations: Vec<VmStorage>,
    /// The VMs' zvols, `<pool>/zendiloos.vms/<id>-disk<N>`, and their sizes.
    zvols: BTreeMap<String, u64>,
    domains: BTreeMap<VmId, Power>,
    /// Domain ids since libvirt started, as `virsh list` numbers them.
    started: u32,
    /// The library's images, the volume each is on and its size.
    images: BTreeMap<VmImageId, (VolumeNumber, u64)>,
    /// Where each upload under way goes.
    upload_volumes: BTreeMap<VmImageId, VolumeNumber>,
    /// Uploads under way: what was announced, and the thread reading.
    pub(crate) uploads: BTreeMap<VmImageId, (u64, Receiving)>,
    /// Looks at the counters, for CPU time that grows.
    looks: u64,
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
                locations: self.locations.clone(),
            },
            Request::EnableVms { .. } if !self.installed => not_installed(),
            Request::EnableVms { pool, volume } => {
                let location = VmStorage { pool: pool.clone(), volume: *volume };
                if !self.locations.contains(&location) {
                    if self.locations.iter().any(|other| other.pool == *pool || other.volume == *volume) {
                        return Some(error(ErrorCode::Forbidden, "refused: the pool is not volume it says"));
                    }
                    self.locations.push(location);
                    self.locations.sort_by_key(|location| location.volume.get());
                }
                Response::Done
            }
            Request::DisableVms { pool, volume } => {
                self.disable(&VmStorage { pool: pool.clone(), volume: *volume })
            }
            Request::VmStorageUsage {} => Response::VmStorageUsage { zfs_list: self.usage() },
            Request::DeleteVm { .. } if self.locations.is_empty() => {
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up")
            }
            Request::CopyVmImage { volume, .. }
            | Request::VmImages { volume }
            | Request::DeleteVmImage { volume, .. }
                if !self.on(*volume) =>
            {
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up there")
            }
            // A small ISO, copied at once.
            Request::CopyVmImage { image, volume, .. } => {
                self.images.insert(image.clone(), (*volume, 4096));
                Response::VmImageStored { bytes: 4096 }
            }
            Request::VmImages { volume } => Response::VmImages {
                files: self
                    .images
                    .iter()
                    .filter(|(_, (on, _))| on == volume)
                    .map(|(image, (_, bytes))| VmImageFile { image: image.clone(), bytes: *bytes })
                    .collect(),
            },
            Request::DeleteVmImage { image, .. } => {
                self.images.remove(image);
                Response::Done
            }
            Request::OpenVmImageUpload { .. } | Request::FinishVmImageUpload { .. } => {
                error(ErrorCode::InvalidRequest, "an upload goes with its pipe")
            }
            Request::DefineVm { .. } | Request::UpdateVm { .. } | Request::ChangeVmMedia { .. }
                if !self.installed =>
            {
                not_installed()
            }
            Request::DefineVm { spec } => {
                if self.domains.contains_key(&spec.id) {
                    error(ErrorCode::Conflict, "the name is already taken")
                } else if let Some(refusal) = self.refuse_devices(spec) {
                    refusal
                } else {
                    self.domains.insert(spec.id.clone(), Power::Off);
                    self.make_disks(spec);
                    Response::Done
                }
            }
            Request::UpdateVm { spec, remove } => match self.domains.get(&spec.id) {
                None => missing(&spec.id),
                Some(Power::Running) => error(
                    ErrorCode::Forbidden,
                    "refused: the virtual machine is not shut off: its hardware changes only while it is off",
                ),
                Some(Power::Off) => match self.refuse_devices(spec) {
                    Some(refusal) => refusal,
                    None => {
                        self.make_disks(spec);
                        for slot in remove {
                            self.remove_disks(&spec.id, Some(*slot));
                        }
                        Response::Done
                    }
                },
            },
            Request::ChangeVmMedia { spec, drive } => {
                if !self.domains.contains_key(&spec.id) {
                    missing(&spec.id)
                } else if spec.drive(*drive).is_none() {
                    error(ErrorCode::Forbidden, &format!("refused: there is no drive {}", drive.get()))
                } else {
                    self.refuse_devices(spec).unwrap_or(Response::Done)
                }
            }
            Request::VmStats {} => Response::VmStats { domstats: self.domstats() },
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
            Request::DeleteVm { id } => {
                self.domains.remove(id);
                self.remove_disks(id, None);
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

    /// Whether a location is on `volume`.
    fn on(&self, volume: VolumeNumber) -> bool {
        self.locations.iter().any(|location| location.volume == volume)
    }

    /// A location removed, once nothing is on it.
    fn disable(&mut self, location: &VmStorage) -> Response {
        if !self.locations.contains(location) {
            return error(
                ErrorCode::Forbidden,
                "refused: there is no location of the virtual machines' storage there",
            );
        }
        let prefix = format!("{}/{VMS_DATASET}/", location.pool.as_str());
        if let Some(zvol) = self.zvols.keys().find(|zvol| zvol.starts_with(&prefix)) {
            return error(
                ErrorCode::Forbidden,
                &format!("refused: it holds the disk {zvol} of a virtual machine"),
            );
        }
        if self.images.values().any(|(volume, _)| *volume == location.volume) {
            return error(ErrorCode::Forbidden, "refused: its image library holds images");
        }
        self.locations.retain(|other| other != location);
        Response::Done
    }

    /// The disks `spec` has and the host not yet, made; those smaller grown.
    fn make_disks(&mut self, spec: &VmSpec) {
        for disk in spec.disks() {
            let zvol = format!("{}/{VMS_DATASET}/{}", disk.location.pool.as_str(), spec.id.disk(disk.slot));
            let size = self.zvols.entry(zvol).or_default();
            *size = (*size).max(disk.size.bytes());
        }
    }

    /// The VM's disks in `slot` on every location, or all of them.
    fn remove_disks(&mut self, id: &VmId, slot: Option<VmDiskSlot>) {
        let name = slot.map(|slot| id.disk(slot));
        self.zvols.retain(|zvol, _| {
            let disk = zvol.rsplit('/').next().unwrap_or_default();
            !(disk.starts_with(&format!("{}-disk", id.as_str()))
                && name.as_deref().is_none_or(|name| name == disk))
        });
    }

    /// `zfs list` of the locations and their zvols: a sparse zvol holds a
    /// quarter of its size, the library what its images have.
    fn usage(&self) -> String {
        let mut text = String::new();
        for location in &self.locations {
            let dataset = format!("{}/{VMS_DATASET}", location.pool.as_str());
            let images: u64 = self
                .images
                .values()
                .filter(|(volume, _)| *volume == location.volume)
                .map(|(_, bytes)| bytes)
                .sum();
            let zvols: Vec<(&String, &u64)> =
                self.zvols.iter().filter(|(zvol, _)| zvol.starts_with(&format!("{dataset}/"))).collect();
            let used = images + zvols.iter().map(|(_, size)| *size / 4).sum::<u64>();
            text.push_str(&format!(
                "{dataset}\tfilesystem\t{used}\t{images}\t{images}\t{}\t-\n",
                400_u64 << 30
            ));
            for (zvol, size) in zvols {
                let held = size / 4;
                text.push_str(&format!(
                    "{zvol}\tvolume\t{held}\t{held}\t{held}\t{}\t{size}\n",
                    400_u64 << 30
                ));
            }
        }
        text
    }

    /// Why the devices could not be defined: a location the storage does
    /// not have, an image not in the library, a bridge the simulated host
    /// does not have (it has `br0`), a card it does not have for a VM to
    /// go on directly (`enp1s0`, `enp2s0`, `bond0`), or an internal network
    /// it does not have (`vmnet0`).
    fn refuse_devices(&self, spec: &VmSpec) -> Option<Response> {
        if let Some(disk) = spec.disks().iter().find(|disk| !self.locations.contains(&disk.location)) {
            return Some(error(
                ErrorCode::Forbidden,
                &format!(
                    "refused: no location of the virtual machines' storage is on pool {}",
                    disk.location.pool.as_str()
                ),
            ));
        }
        if let Some(image) = spec.images().find(|image| !self.images.contains_key(*image)) {
            return Some(error(
                ErrorCode::Forbidden,
                &format!("refused: the image {} is not in the library", image.as_str()),
            ));
        }
        spec.nics().iter().find_map(|nic| match &nic.network {
            VmNetwork::Bridge { bridge } if bridge.as_str() != "br0" => Some(error(
                ErrorCode::Forbidden,
                &format!("refused: {} is no bridge of this machine", bridge.as_str()),
            )),
            VmNetwork::Direct { card } if !["enp1s0", "enp2s0", "bond0"].contains(&card.as_str()) => {
                Some(error(
                    ErrorCode::Forbidden,
                    &format!("refused: {} is no wired card or bond of this machine", card.as_str()),
                ))
            }
            VmNetwork::Internal { bridge } if bridge.as_str() != "vmnet0" => Some(error(
                ErrorCode::Forbidden,
                &format!("refused: {} is no internal network the NAS keeps itself off", bridge.as_str()),
            )),
            _ => None,
        })
    }

    /// `virsh domstats` of the running domains: CPU time that grows by a
    /// second's worth of one vCPU at each look, a guest using half of 1 GiB,
    /// and network counters that grow too.
    fn domstats(&mut self) -> String {
        self.looks += 1;
        let mut text = String::new();
        for (id, power) in &self.domains {
            if *power != Power::Running {
                continue;
            }
            let n = self.looks;
            text.push_str(&format!(
                "Domain: '{}'
  state.state=1
  cpu.time={}
  balloon.current=1048576
                   balloon.maximum=1048576
  balloon.available=1000000
  balloon.usable=500000
                   balloon.rss=600000
  net.count=1
  net.0.name=vnet0
  net.0.rx.bytes={}
                   net.0.tx.bytes={}

",
                id.domain(),
                n * 1_000_000_000,
                n * 1_000_000,
                n * 250_000
            ));
        }
        text
    }

    /// An upload as the agent takes it: a pipe whose reading end a thread
    /// drains, counting.
    pub(crate) fn open_upload(
        &mut self,
        image: &VmImageId,
        volume: VolumeNumber,
        bytes: u64,
    ) -> (Response, Option<OwnedFd>) {
        if !self.on(volume) {
            return (
                error(ErrorCode::Forbidden, "refused: the virtual machines' storage is not set up"),
                None,
            );
        }
        if bytes == 0 || bytes > VM_IMAGE_UPLOAD_MAX {
            return (error(ErrorCode::Forbidden, "refused: an uploaded image has 1 byte to 32 GiB"), None);
        }
        if self.images.contains_key(image) || self.uploads.contains_key(image) {
            return (error(ErrorCode::Conflict, "the name is already taken"), None);
        }
        let (mut reader, writer) = match std::io::pipe() {
            Ok(pipe) => pipe,
            Err(e) => return (error(ErrorCode::Failed, &format!("no pipe: {e}")), None),
        };
        self.upload_volumes.insert(image.clone(), volume);
        let thread = std::thread::spawn(move || {
            let mut sink = [0u8; 64 * 1024];
            let mut received = 0u64;
            while let Ok(read) = reader.read(&mut sink) {
                if read == 0 {
                    break;
                }
                received += read as u64;
            }
            received
        });
        self.uploads.insert(image.clone(), (bytes, thread));
        (Response::VmImageUpload {}, Some(OwnedFd::from(writer)))
    }

    /// What an upload whose thread ended stored.
    pub(crate) fn finish_upload(&mut self, image: &VmImageId, announced: u64, received: u64) -> Response {
        let volume = self.upload_volumes.remove(image);
        if let (Some(volume), true) = (volume, received == announced) {
            self.images.insert(image.clone(), (volume, received));
            Response::VmImageStored { bytes: received }
        } else {
            error(
                ErrorCode::Forbidden,
                &format!("refused: the upload ended after {received} of {announced} bytes"),
            )
        }
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
