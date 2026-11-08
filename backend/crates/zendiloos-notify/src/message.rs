//! Notifications in words, for channels that leave the interface (email,
//! webhooks). The interface words them itself; these texts follow it.

use crate::Notification;

/// The language a channel's messages are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    German,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "en" => Some(Self::English),
            "de" => Some(Self::German),
            _ => None,
        }
    }
}

/// A notification as a subject line and a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
}

/// Words `notification` from the NAS `host` in `language`.
pub fn message(notification: &Notification, host: &str, language: Language) -> Message {
    let param = |key: &str| notification.params.get(key).map_or("", String::as_str);
    let errors: u64 = param("errors").parse().unwrap_or(0);
    let german = language == Language::German;
    let (title, body) = match notification.code.as_str() {
        "pool_trouble" => {
            let state = pool_state(param("state"), german);
            if german {
                (
                    format!("Pool {} ist {state}", param("pool")),
                    "Unter Speicher sehen Sie, welche Platte betroffen ist.".to_owned(),
                )
            } else {
                (
                    format!("Pool {} is {state}", param("pool")),
                    "Open Storage to see which disk is affected.".to_owned(),
                )
            }
        }
        "pool_recovered" if german => (format!("Pool {} ist wieder intakt", param("pool")), String::new()),
        "pool_recovered" => (format!("Pool {} is healthy again", param("pool")), String::new()),
        "resilver_finished" if german => (
            format!("Pool {} ist wiederhergestellt", param("pool")),
            count(errors, "Keine Fehler.", "Ein Fehler wurde gefunden.", "Fehler wurden gefunden."),
        ),
        "resilver_finished" => (
            format!("Pool {} is rebuilt", param("pool")),
            count(errors, "No errors.", "One error was found.", "errors were found."),
        ),
        "scrub_finished" if german => (
            format!("Datenprüfung von {} abgeschlossen", param("pool")),
            count(
                errors,
                "Keine Fehler.",
                "Ein Fehler wurde gefunden und behoben.",
                "Fehler wurden gefunden und behoben.",
            ),
        ),
        "scrub_finished" => (
            format!("Data check of {} finished", param("pool")),
            count(
                errors,
                "No errors.",
                "One error was found and repaired.",
                "errors were found and repaired.",
            ),
        ),
        "volume_filling" if german => (
            format!("Pool {} ist zu {} % voll", param("pool"), param("percent")),
            "Geben Sie Platz frei oder erweitern Sie, bevor er voll ist.".to_owned(),
        ),
        "volume_filling" => (
            format!("Pool {} is {} % full", param("pool"), param("percent")),
            "Free up space or add disks before it is full.".to_owned(),
        ),
        "disk_health" => {
            let failing = param("health") == "failing";
            let model = match param("model") {
                "" => param("disk"),
                model => model,
            };
            if german {
                let health = if failing { "kritisch" } else { "auffällig" };
                (
                    format!("Platte {} meldet Probleme", param("disk")),
                    format!("{model}: SMART-Zustand ist {health}. Planen Sie den Austausch."),
                )
            } else {
                let health = if failing { "failing" } else { "worrying" };
                (
                    format!("Disk {} reports problems", param("disk")),
                    format!("{model}: SMART health is {health}. Plan to replace it."),
                )
            }
        }
        "snapshot_failed" if german => (
            format!("Kein Snapshot von {}", param("share")),
            "Ein geplanter Snapshot konnte nicht aufgenommen werden. Ist das Volume der Freigabe da?"
                .to_owned(),
        ),
        "snapshot_failed" => (
            format!("No snapshot of {}", param("share")),
            "A scheduled snapshot could not be taken. Is the share's volume there?".to_owned(),
        ),
        "removable_pulled" if german => (
            format!("{} wurde abgezogen, ohne ausgeworfen zu werden", param("drive")),
            "Werfen Sie Laufwerke aus, bevor Sie sie abziehen; was gerade geschrieben wurde, kann fehlen."
                .to_owned(),
        ),
        "removable_pulled" => (
            format!("{} was pulled out without being ejected", param("drive")),
            "Eject drives before pulling them out; what was being written may be missing.".to_owned(),
        ),
        "updates_available" => {
            let (packages, security, version) = (param("packages"), param("security"), param("version"));
            let body = match (german, version) {
                (true, "") => format!("{packages} Debian-Pakete, davon {security} Sicherheitsupdates."),
                (true, version) => {
                    format!(
                        "zendiloOS {version} und {packages} Debian-Pakete, davon {security} Sicherheitsupdates."
                    )
                }
                (false, "") => format!("{packages} Debian packages, {security} of them security updates."),
                (false, version) => {
                    format!(
                        "zendiloOS {version} and {packages} Debian packages, {security} of them security updates."
                    )
                }
            };
            let title = if german { "Updates verfügbar" } else { "Updates available" };
            (title.to_owned(), body)
        }
        "updates_installed" => {
            let body = match (german, param("zfs_ready") == "false", param("restart") == "true") {
                (true, true, _) => {
                    "Das ZFS-Modul fehlt für den neuen Kernel. Starten Sie das NAS nicht neu, bevor es gebaut ist."
                }
                (false, true, _) => {
                    "The ZFS module is missing for the new kernel. Do not restart the NAS before it is built."
                }
                (true, false, true) => "Ein neuer Kernel wartet auf einen Neustart des NAS.",
                (false, false, true) => "A new kernel waits for the NAS to restart.",
                (true, false, false) => "zendiloOS und die Debian-Pakete sind aktuell.",
                (false, false, false) => "zendiloOS and the Debian packages are current.",
            };
            let title = if german { "Updates installiert" } else { "Updates installed" };
            (title.to_owned(), body.to_owned())
        }
        "updates_failed" if german => (
            "Update fehlgeschlagen".to_owned(),
            "Unter Einstellungen → Aktualisieren & Wiederherstellen steht, woran es lag.".to_owned(),
        ),
        "updates_failed" => ("Update failed".to_owned(), "Settings → Update & restore says why.".to_owned()),
        "certificate_renewed" if german => (
            format!("Zertifikat für {} erneuert", param("name")),
            "Let's Encrypt hat das Zertifikat erneuert; zeigt die Weboberfläche es, nutzt sie es schon."
                .to_owned(),
        ),
        "certificate_renewed" => (
            format!("Certificate for {} renewed", param("name")),
            "Let's Encrypt renewed the certificate; if the web interface shows it, it uses it already."
                .to_owned(),
        ),
        "certificate_renewal_failed" if german => (
            format!("Zertifikat für {} nicht erneuert", param("name")),
            format!(
                "{}\nDas bisherige gilt weiter, bis es abläuft; morgen wird es erneut versucht.",
                param("reason")
            ),
        ),
        "certificate_renewal_failed" => (
            format!("Certificate for {} not renewed", param("name")),
            format!("{}\nThe current one holds until it ends; it is tried again tomorrow.", param("reason")),
        ),
        "channel_test" if german => (
            "Testnachricht".to_owned(),
            "Dieser Kanal erreicht Sie. So kommen Meldungen des NAS an.".to_owned(),
        ),
        "channel_test" => (
            "Test message".to_owned(),
            "This channel reaches you. This is how the NAS will tell you.".to_owned(),
        ),
        // A code this version does not word yet: its parameters.
        code => {
            let params: Vec<String> = notification.params.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            (code.replace('_', " "), params.join("\n"))
        }
    };
    Message { title: format!("[{host}] {title}"), body }
}

fn count(errors: u64, none: &str, one: &str, many: &str) -> String {
    match errors {
        0 => none.to_owned(),
        1 => one.to_owned(),
        n => format!("{n} {many}"),
    }
}

fn pool_state(state: &str, german: bool) -> &str {
    match (state, german) {
        ("degraded", false) => "degraded",
        ("degraded", true) => "beeinträchtigt",
        ("faulted", false) => "failed",
        ("faulted", true) => "ausgefallen",
        ("unavailable", false) => "unavailable",
        ("unavailable", true) => "nicht verfügbar",
        ("suspended", false) => "suspended",
        ("suspended", true) => "angehalten",
        ("offline", _) => "offline",
        ("removed", false) => "removed",
        ("removed", true) => "entfernt",
        (_, false) => "in an unknown state",
        (_, true) => "in einem unbekannten Zustand",
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Timestamp;

    use super::*;
    use crate::Level;

    fn notification(code: &str, params: &[(&str, &str)]) -> Notification {
        Notification {
            id: 1,
            at: Timestamp::from_unix_seconds(0),
            level: Level::Warning,
            code: code.to_owned(),
            params: params.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        }
    }

    #[test]
    fn notifications_are_worded_as_in_the_interface() {
        let degraded = notification("pool_trouble", &[("pool", "tank"), ("state", "degraded")]);
        assert_eq!(
            message(&degraded, "nas", Language::English),
            Message {
                title: "[nas] Pool tank is degraded".into(),
                body: "Open Storage to see which disk is affected.".into()
            }
        );
        assert_eq!(message(&degraded, "nas", Language::German).title, "[nas] Pool tank ist beeinträchtigt");
        let scrub = notification("scrub_finished", &[("pool", "tank"), ("errors", "3")]);
        assert_eq!(message(&scrub, "nas", Language::English).body, "3 errors were found and repaired.");
        let disk =
            notification("disk_health", &[("disk", "sda"), ("model", "WD RED"), ("health", "failing")]);
        assert_eq!(
            message(&disk, "nas", Language::German).body,
            "WD RED: SMART-Zustand ist kritisch. Planen Sie den Austausch."
        );
    }

    #[test]
    fn every_known_code_is_worded_in_both_languages() {
        let cases = [
            (
                "pool_recovered",
                vec![("pool", "tank")],
                "Pool tank is healthy again",
                "Pool tank ist wieder intakt",
            ),
            (
                "resilver_finished",
                vec![("pool", "tank")],
                "Pool tank is rebuilt",
                "Pool tank ist wiederhergestellt",
            ),
            (
                "scrub_finished",
                vec![("pool", "tank")],
                "Data check of tank finished",
                "Datenprüfung von tank abgeschlossen",
            ),
            (
                "volume_filling",
                vec![("pool", "tank"), ("percent", "91")],
                "Pool tank is 91 % full",
                "Pool tank ist zu 91 % voll",
            ),
            (
                "disk_health",
                vec![("disk", "sda"), ("health", "warning")],
                "Disk sda reports problems",
                "Platte sda meldet Probleme",
            ),
            (
                "snapshot_failed",
                vec![("share", "photos")],
                "No snapshot of photos",
                "Kein Snapshot von photos",
            ),
            ("channel_test", vec![], "Test message", "Testnachricht"),
            (
                "certificate_renewed",
                vec![("name", "nas.example.org")],
                "Certificate for nas.example.org renewed",
                "Zertifikat für nas.example.org erneuert",
            ),
            (
                "certificate_renewal_failed",
                vec![("name", "nas.example.org"), ("reason", "acme.sh: Verify error")],
                "Certificate for nas.example.org not renewed",
                "Zertifikat für nas.example.org nicht erneuert",
            ),
            (
                "removable_pulled",
                vec![("drive", "STICK")],
                "STICK was pulled out without being ejected",
                "STICK wurde abgezogen, ohne ausgeworfen zu werden",
            ),
        ];
        for (code, params, english, german) in cases {
            let notification = notification(code, &params);
            let (en, de) = (
                message(&notification, "nas", Language::English),
                message(&notification, "nas", Language::German),
            );
            assert_eq!(en.title, format!("[nas] {english}"));
            assert_eq!(de.title, format!("[nas] {german}"));
        }
        let worrying = notification("disk_health", &[("disk", "sda"), ("health", "warning")]);
        assert_eq!(
            message(&worrying, "nas", Language::English).body,
            "sda: SMART health is worrying. Plan to replace it."
        );
        assert!(message(&worrying, "nas", Language::German).body.contains("auffällig"));
    }

    #[test]
    fn updates_are_worded_with_what_they_bring() {
        let body = |code, params: &[(&str, &str)], language| {
            message(&notification(code, params), "nas", language).body
        };
        let debian = [("packages", "12"), ("security", "3")];
        assert_eq!(
            body("updates_available", &debian, Language::English),
            "12 Debian packages, 3 of them security updates."
        );
        let both = [("packages", "12"), ("security", "3"), ("version", "0.2.0")];
        assert_eq!(
            body("updates_available", &both, Language::German),
            "zendiloOS 0.2.0 und 12 Debian-Pakete, davon 3 Sicherheitsupdates."
        );
        assert!(body("updates_available", &both, Language::English).starts_with("zendiloOS 0.2.0 and"));
        assert!(body("updates_available", &debian, Language::German).starts_with("12 Debian-Pakete"));
        let current = [("restart", "false"), ("zfs_ready", "true")];
        assert_eq!(
            body("updates_installed", &current, Language::English),
            "zendiloOS and the Debian packages are current."
        );
        assert!(body("updates_installed", &current, Language::German).contains("aktuell"));
        let kernel = [("restart", "true"), ("zfs_ready", "true")];
        assert!(body("updates_installed", &kernel, Language::English).contains("restart"));
        assert!(body("updates_installed", &kernel, Language::German).contains("Neustart"));
        let no_zfs = [("restart", "true"), ("zfs_ready", "false")];
        assert!(body("updates_installed", &no_zfs, Language::English).contains("ZFS module is missing"));
        assert!(body("updates_installed", &no_zfs, Language::German).contains("ZFS-Modul fehlt"));
        let failed = notification("updates_failed", &[("error", "apt")]);
        assert_eq!(message(&failed, "nas", Language::English).title, "[nas] Update failed");
        assert_eq!(message(&failed, "nas", Language::German).title, "[nas] Update fehlgeschlagen");
        assert_eq!(
            message(&notification("updates_installed", &current), "nas", Language::German).title,
            "[nas] Updates installiert"
        );
        assert_eq!(
            message(&notification("updates_available", &debian), "nas", Language::English).title,
            "[nas] Updates available"
        );
    }

    #[test]
    fn errors_are_counted_in_words() {
        for (errors, english, german) in [
            ("0", "No errors.", "Keine Fehler."),
            ("1", "One error was found.", "Ein Fehler wurde gefunden."),
            ("4", "4 errors were found.", "4 Fehler wurden gefunden."),
        ] {
            let rebuilt = notification("resilver_finished", &[("pool", "tank"), ("errors", errors)]);
            assert_eq!(message(&rebuilt, "nas", Language::English).body, english);
            assert_eq!(message(&rebuilt, "nas", Language::German).body, german);
        }
        let scrubbed = notification("scrub_finished", &[("pool", "tank"), ("errors", "1")]);
        assert_eq!(
            message(&scrubbed, "nas", Language::German).body,
            "Ein Fehler wurde gefunden und behoben."
        );
    }

    #[test]
    fn a_pools_state_is_worded_and_the_language_named() {
        for (state, english, german) in [
            ("degraded", "degraded", "beeinträchtigt"),
            ("faulted", "failed", "ausgefallen"),
            ("unavailable", "unavailable", "nicht verfügbar"),
            ("suspended", "suspended", "angehalten"),
            ("offline", "offline", "offline"),
            ("removed", "removed", "entfernt"),
            ("weird", "in an unknown state", "in einem unbekannten Zustand"),
        ] {
            assert_eq!((pool_state(state, false), pool_state(state, true)), (english, german));
        }
        for language in [Language::English, Language::German] {
            assert_eq!(Language::parse(language.as_str()), Some(language));
        }
        assert_eq!(Language::parse("fr"), None);
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn unknown_codes_still_say_what_happened() {
        let odd = notification("pool_on_fire", &[("pool", "tank")]);
        assert_eq!(
            message(&odd, "nas", Language::English),
            Message { title: "[nas] pool on fire".into(), body: "pool: tank".into() }
        );
    }
}
