/// Static facts about the host, plus its uptime. A value object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub hostname: String,
    /// `PRETTY_NAME` from os-release, e.g. `Debian GNU/Linux 13 (trixie)`.
    pub os_name: Option<String>,
    /// Kernel release, e.g. `6.12.48+deb13-amd64`.
    pub kernel_release: String,
    /// Machine architecture, e.g. `x86_64` or `aarch64`.
    pub architecture: String,
    /// CPU model as reported by the kernel, e.g. `Intel(R) N100`.
    pub cpu_model: Option<String>,
    /// Logical CPUs (hardware threads).
    pub cpu_threads: usize,
    /// Physical cores, if the kernel says.
    pub cpu_cores: Option<usize>,
    pub memory_total_bytes: u64,
    pub swap_total_bytes: u64,
    pub uptime_seconds: u64,
    pub hardware: Hardware,
}

/// What the firmware says about the machine (DMI). Placeholders some
/// boards ship with are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hardware {
    /// E.g. `ASUSTeK COMPUTER INC.`
    pub vendor: Option<String>,
    /// E.g. `PN42`.
    pub product: Option<String>,
    /// Mainboard maker and name.
    pub board: Option<String>,
    /// Firmware maker, version and date, e.g. `American Megatrends 0304 (05/12/2024)`.
    pub firmware: Option<String>,
    /// Booted through UEFI rather than a legacy BIOS.
    pub uefi: bool,
}

/// `value` unless it is empty or one of the fillers boards ship with.
pub fn meaningful(value: &str) -> Option<String> {
    const FILLERS: [&str; 8] = [
        "to be filled by o.e.m.",
        "default string",
        "system product name",
        "system manufacturer",
        "not specified",
        "not applicable",
        "none",
        "o.e.m.",
    ];
    let value = value.trim();
    (!value.is_empty() && !FILLERS.contains(&value.to_lowercase().as_str())).then(|| value.to_owned())
}

/// Parts joined by spaces, leaving out those that say nothing.
pub fn joined(parts: &[Option<String>]) -> Option<String> {
    let parts: Vec<&str> = parts.iter().flatten().map(String::as_str).collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fillers_say_nothing() {
        assert_eq!(meaningful(" ASRock \n").as_deref(), Some("ASRock"));
        for filler in ["", "  ", "To Be Filled By O.E.M.", "Default string", "System Product Name"] {
            assert_eq!(meaningful(filler), None, "{filler:?}");
        }
        assert_eq!(
            joined(&[Some("SeaBIOS".into()), None, Some("1.16".into())]).as_deref(),
            Some("SeaBIOS 1.16")
        );
        assert_eq!(joined(&[None, None]), None);
    }
}
