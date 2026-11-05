/// Restarting or switching off the whole system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    PowerOff,
}

impl PowerAction {
    /// Name in the audit log.
    pub fn audit_action(self) -> &'static str {
        match self {
            PowerAction::Reboot => "system.reboot",
            PowerAction::PowerOff => "system.power_off",
        }
    }
}
