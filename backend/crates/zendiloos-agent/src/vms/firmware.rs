//! Secure Boot with Microsoft's certificates of 2023 as well as 2011's.
//!
//! Debian 13's OVMF with Microsoft's keys (`OVMF_VARS_4M.ms.fd`) enrols the
//! certificates of 2011 only. A boot loader signed only with one of 2023,
//! as newer Windows media and shims are, is refused with `Access Denied`.
//! So zendiloOS makes a template of its own: Debian's with Microsoft's KEK
//! of 2023 and its two db certificates of 2023 added by virt-fw-vars
//! (python3-virt-firmware, which ships them), and describes it to QEMU's
//! firmware selection in `/etc/qemu/firmware`, before Debian's description
//! and otherwise the same. libvirt picks it when a domain with Secure Boot
//! is defined and makes a VM's variables from it at the VM's first start,
//! or when they start afresh. Variables a VM has already stay as they are.
//!
//! The template is made anew whenever Debian's template or the
//! certificates change, and goes with its description once Debian's is
//! gone. Without virt-fw-vars nothing is made, and libvirt takes Debian's.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zendiloos_exec::Program;

use super::{ENROLLED, MAX_DESCRIPTOR, QUICK, run};
use crate::operations::OperationError;

/// The owner Microsoft's certificates are enrolled with.
const MICROSOFT: &str = "77fa9abd-0359-4d32-bd60-28f4e78f784b";
/// Microsoft's KEK of 2023, as virt-firmware names its file.
const KEK_2023: &str = "MicrosoftCorporationKEK2KCA2023.pem";
/// The db's certificates of 2023: Windows' boot manager's, and the one of
/// other loaders (shim).
const DB_2023: [&str; 2] = ["WindowsUEFICA2023.pem", "MicrosoftUEFICA2023.pem"];
/// The three as their subjects name them, which the variables hold once
/// they are enrolled.
pub(crate) const NAMES_2023: [&str; 3] =
    ["Microsoft Corporation KEK 2K CA 2023", "Windows UEFI CA 2023", "Microsoft UEFI CA 2023"];
/// zendiloOS's description, which sorts before Debian's `40-…`.
pub(crate) const DESCRIPTION: &str = "30-zendiloos-x86_64-secure-enrolled.json";
/// The template's name, beside what it was made from.
const TEMPLATE: &str = "OVMF_VARS_4M.ms.fd";
const SOURCE: &str = "OVMF_VARS_4M.ms.fd.source";
/// A template is this small (Debian's has 528 KiB).
const MAX_TEMPLATE: u64 = 64 * 1024 * 1024;

/// Where the descriptions and templates are.
#[derive(Debug, Clone)]
pub(crate) struct FirmwarePlaces<'a> {
    /// Debian's descriptions, `/usr/share/qemu/firmware`.
    pub debian: &'a Path,
    /// The host's own, `/etc/qemu/firmware`.
    pub own: &'a Path,
    /// Where zendiloOS keeps its template.
    pub variables: &'a Path,
    /// virt-firmware's certificates.
    pub certificates: &'a Path,
}

/// Debian's description of OVMF with Microsoft's keys and Secure Boot,
/// the one libvirt would pick, and its template.
#[derive(Debug)]
struct Enrolled {
    description: serde_json::Value,
    template: PathBuf,
}

/// Whether virt-fw-vars and the certificates of 2023 are there.
pub(crate) fn can_enrol(places: &FirmwarePlaces<'_>, virt_fw_vars: &Program) -> bool {
    virt_fw_vars.path().exists() && certificates(places).iter().all(|path| path.is_file())
}

fn certificates(places: &FirmwarePlaces<'_>) -> Vec<PathBuf> {
    std::iter::once(KEK_2023).chain(DB_2023).map(|name| places.certificates.join(name)).collect()
}

/// Makes the template and its description, or makes them anew, as Debian's
/// template and the certificates are now; removes them once Debian's
/// description is gone. Returns whether a domain defined now gets the
/// certificates of 2023.
pub(crate) async fn ensure(
    places: &FirmwarePlaces<'_>,
    virt_fw_vars: &Program,
) -> Result<bool, OperationError> {
    let own = places.own.join(DESCRIPTION);
    let Some(enrolled) = debians(places.debian) else {
        remove(&own)?;
        remove(&places.variables.join(TEMPLATE))?;
        remove(&places.variables.join(SOURCE))?;
        return Ok(false);
    };
    if !can_enrol(places, virt_fw_vars) {
        // Made before, it stays: what libvirt recorded for a VM still
        // names it.
        return Ok(own.is_file());
    }
    let template = places.variables.join(TEMPLATE);
    let source = digest(&enrolled.template, &certificates(places))?;
    let recorded = fs::read_to_string(places.variables.join(SOURCE)).unwrap_or_default();
    if recorded.trim() != source || !template.is_file() {
        make_template(places, virt_fw_vars, &enrolled.template, &template).await?;
        crate::durable::replace(&places.variables.join(SOURCE), format!("{source}\n").as_bytes())?;
    }
    let mut description = enrolled.description;
    description["mapping"]["nvram-template"]["filename"] = template.to_string_lossy().into_owned().into();
    let said = description["description"].as_str().unwrap_or("UEFI firmware with Secure Boot").to_owned();
    description["description"] =
        format!("{said}; Microsoft's certificates of 2023 enrolled too, by zendiloOS").into();
    let mut text = serde_json::to_string_pretty(&description)
        .map_err(|e| OperationError::Failed(format!("the firmware's description: {e}")))?;
    text.push('\n');
    fs::create_dir_all(places.own).map_err(|e| OperationError::Write(places.own.to_owned(), e))?;
    crate::durable::replace_if_changed(&own, text.as_bytes())?;
    Ok(true)
}

/// Debian's template with Microsoft's certificates of 2023 added, checked
/// to hold them.
async fn make_template(
    places: &FirmwarePlaces<'_>,
    virt_fw_vars: &Program,
    from: &Path,
    to: &Path,
) -> Result<(), OperationError> {
    fs::create_dir_all(places.variables)
        .map_err(|e| OperationError::Write(places.variables.to_owned(), e))?;
    let made = places.variables.join(format!(".{TEMPLATE}.made"));
    let _ = fs::remove_file(&made);
    let mut args = vec!["--input".to_owned(), from.to_string_lossy().into_owned()];
    args.extend(["--output".to_owned(), made.to_string_lossy().into_owned()]);
    let kek = places.certificates.join(KEK_2023);
    args.extend(["--add-kek".to_owned(), MICROSOFT.to_owned(), kek.to_string_lossy().into_owned()]);
    for name in DB_2023 {
        let cert = places.certificates.join(name);
        args.extend(["--add-db".to_owned(), MICROSOFT.to_owned(), cert.to_string_lossy().into_owned()]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let ran = run(virt_fw_vars, &args, QUICK).await;
    let content = ran.and_then(|()| read(&made, MAX_TEMPLATE));
    let _ = fs::remove_file(&made);
    let content = content?;
    if let Some(missing) = NAMES_2023.iter().find(|name| !holds(&content, name)) {
        return Err(OperationError::Failed(format!("virt-fw-vars left {missing} out of the UEFI variables")));
    }
    crate::durable::replace_mode(to, &content, 0o644)
}

/// Whether the variables hold a certificate of that subject.
pub(crate) fn holds(variables: &[u8], name: &str) -> bool {
    variables.windows(name.len()).any(|window| window == name.as_bytes())
}

/// The first description by name, as libvirt orders them, of OVMF for a
/// PC with Microsoft's keys and Secure Boot, in raw flash with a template.
fn debians(folder: &Path) -> Option<Enrolled> {
    let mut names: Vec<PathBuf> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    names.sort();
    names.into_iter().find_map(|path| {
        let text = String::from_utf8(read(&path, MAX_DESCRIPTOR).ok()?).ok()?;
        if !ENROLLED.iter().all(|feature| text.contains(feature)) {
            return None;
        }
        let description: serde_json::Value = serde_json::from_str(&text).ok()?;
        let mapping = &description["mapping"];
        let for_a_pc =
            description["targets"].as_array()?.iter().any(|target| target["architecture"] == "x86_64");
        let raw = |part: &str| mapping[part]["format"].as_str().is_none_or(|format| format == "raw");
        let template = PathBuf::from(mapping["nvram-template"]["filename"].as_str()?);
        (for_a_pc
            && mapping["device"] == "flash"
            && raw("executable")
            && raw("nvram-template")
            && template.is_file())
        .then_some(Enrolled { description, template })
    })
}

/// What the template is made from: Debian's and the certificates.
fn digest(template: &Path, certificates: &[PathBuf]) -> Result<String, OperationError> {
    let mut hash = Sha256::new();
    for path in std::iter::once(template).chain(certificates.iter().map(PathBuf::as_path)) {
        let content = read(path, MAX_TEMPLATE)?;
        hash.update((content.len() as u64).to_le_bytes());
        hash.update(content);
    }
    Ok(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read(path: &Path, max: u64) -> Result<Vec<u8>, OperationError> {
    let mut content = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(max + 1).read_to_end(&mut content))
        .map_err(|e| OperationError::Failed(format!("{}: {e}", path.display())))?;
    if content.len() as u64 > max {
        return Err(OperationError::Failed(format!("{} is too large", path.display())));
    }
    Ok(content)
}

fn remove(path: &Path) -> Result<(), OperationError> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(OperationError::Write(path.to_owned(), error))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Debian's description of OVMF with Microsoft's keys, as in ovmf
    /// 2025.02-8+deb13u1, its template beside it.
    fn debian(root: &Path) -> PathBuf {
        let folder = root.join("share");
        fs::create_dir_all(&folder).unwrap();
        let template = root.join("OVMF_VARS_4M.ms.fd");
        fs::write(&template, b"variables: Microsoft Corporation KEK CA 2011").unwrap();
        let description = serde_json::json!({
            "description": "UEFI firmware for x86_64, with Secure Boot, required SMM, SB enabled, MS certs enrolled",
            "interface-types": ["uefi"],
            "mapping": {
                "device": "flash",
                "executable": {"filename": "/usr/share/OVMF/OVMF_CODE_4M.ms.fd", "format": "raw"},
                "nvram-template": {"filename": template, "format": "raw"},
            },
            "targets": [{"architecture": "x86_64", "machines": ["pc-q35-*"]}],
            "features": ["acpi-s3", "amd-sev", "enrolled-keys", "requires-smm", "secure-boot", "verbose-dynamic"],
            "tags": [],
        });
        fs::write(folder.join("40-edk2-x86_64-secure-enrolled.json"), description.to_string()).unwrap();
        // Without Microsoft's keys: never the one.
        fs::write(folder.join("30-edk2-x86_64-secure.json"), r#"{"features": ["secure-boot"]}"#).unwrap();
        folder
    }

    /// virt-fw-vars as a script: copies its input and adds the subjects of
    /// what it was to add, unless `leave-out` names one; logs its calls.
    fn fake(root: &Path) -> Program {
        let path = root.join("virt-fw-vars");
        let script = format!(
            r#"#!/bin/sh
echo "$@" >> {log}
while [ $# -gt 0 ]; do
    case $1 in
        --input) input=$2; shift 2 ;;
        --output) output=$2; shift 2 ;;
        --add-kek|--add-db) added="$added $3"; shift 3 ;;
        *) shift ;;
    esac
done
cp "$input" "$output"
for cert in $added; do
    case $cert in
        *KEK2KCA2023*) name="Microsoft Corporation KEK 2K CA 2023" ;;
        *WindowsUEFICA2023*) name="Windows UEFI CA 2023" ;;
        *) name="Microsoft UEFI CA 2023" ;;
    esac
    [ -e {root}/leave-out ] && [ "$name" = "$(cat {root}/leave-out)" ] && continue
    printf '%s' "$name" >> "$output"
done
"#,
            log = root.join("calls.log").display(),
            root = root.display(),
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        Program::new(&path).unwrap()
    }

    fn certs(root: &Path) -> PathBuf {
        let folder = root.join("certs");
        fs::create_dir_all(&folder).unwrap();
        for name in std::iter::once(KEK_2023).chain(DB_2023) {
            fs::write(folder.join(name), format!("-----BEGIN CERTIFICATE----- {name}")).unwrap();
        }
        folder
    }

    fn calls(root: &Path) -> usize {
        fs::read_to_string(root.join("calls.log")).map_or(0, |log| log.lines().count())
    }

    #[tokio::test]
    async fn the_template_has_microsofts_certificates_of_2023_and_is_described_before_debians() {
        let root = tempfile::tempdir().unwrap();
        let (debian, certificates) = (debian(root.path()), certs(root.path()));
        let (own, variables) = (root.path().join("etc"), root.path().join("state"));
        let places =
            FirmwarePlaces { debian: &debian, own: &own, variables: &variables, certificates: &certificates };
        let tool = fake(root.path());
        assert!(can_enrol(&places, &tool));

        assert!(ensure(&places, &tool).await.unwrap());
        let template = fs::read(variables.join(TEMPLATE)).unwrap();
        assert!(holds(&template, "Microsoft Corporation KEK CA 2011"), "Debian's stay");
        assert!(NAMES_2023.iter().all(|name| holds(&template, name)));
        let log = fs::read_to_string(root.path().join("calls.log")).unwrap();
        assert!(
            log.contains(&format!("--add-kek {MICROSOFT} {}", certificates.join(KEK_2023).display())),
            "the KEK of 2023 is added with Microsoft as its owner"
        );
        let described: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(own.join(DESCRIPTION)).unwrap()).unwrap();
        assert_eq!(
            described["mapping"]["nvram-template"]["filename"],
            variables.join(TEMPLATE).to_str().unwrap()
        );
        assert_eq!(described["mapping"]["executable"]["filename"], "/usr/share/OVMF/OVMF_CODE_4M.ms.fd");
        assert!(described["features"].as_array().unwrap().iter().any(|feature| feature == "enrolled-keys"));
        assert!(described["description"].as_str().unwrap().contains("2023"));
        assert!(DESCRIPTION < "40-edk2-x86_64-secure-enrolled.json", "libvirt takes the first by name");

        // Nothing changed: nothing is made again.
        assert!(ensure(&places, &tool).await.unwrap());
        assert_eq!(calls(root.path()), 1);

        // Debian's template changed (an update of ovmf): made anew from it.
        let theirs = root.path().join("OVMF_VARS_4M.ms.fd");
        fs::write(&theirs, b"newer variables: Microsoft Corporation KEK CA 2011").unwrap();
        ensure(&places, &tool).await.unwrap();
        assert_eq!(calls(root.path()), 2);
        assert!(holds(&fs::read(variables.join(TEMPLATE)).unwrap(), "newer variables"));

        // ovmf removed: the description and the template go.
        fs::remove_file(debian.join("40-edk2-x86_64-secure-enrolled.json")).unwrap();
        assert!(!ensure(&places, &tool).await.unwrap());
        assert!(!own.join(DESCRIPTION).exists() && !variables.join(TEMPLATE).exists());
    }

    #[tokio::test]
    async fn without_virt_firmware_nothing_is_made_and_a_certificate_left_out_is_no_template() {
        let root = tempfile::tempdir().unwrap();
        let (debian, certificates) = (debian(root.path()), certs(root.path()));
        let (own, variables) = (root.path().join("etc"), root.path().join("state"));
        let places =
            FirmwarePlaces { debian: &debian, own: &own, variables: &variables, certificates: &certificates };
        let missing = Program::new(root.path().join("no-virt-fw-vars")).unwrap();
        assert!(!can_enrol(&places, &missing));
        assert!(!ensure(&places, &missing).await.unwrap());
        assert!(!own.join(DESCRIPTION).exists());

        let tool = fake(root.path());
        fs::write(root.path().join("leave-out"), "Windows UEFI CA 2023").unwrap();
        let failed = ensure(&places, &tool).await.unwrap_err();
        assert!(failed.to_string().contains("left Windows UEFI CA 2023 out"), "{failed}");
        assert!(!variables.join(TEMPLATE).exists() && !own.join(DESCRIPTION).exists());
        assert!(fs::read_dir(&variables).unwrap().next().is_none(), "nothing half made stays");
    }
}
