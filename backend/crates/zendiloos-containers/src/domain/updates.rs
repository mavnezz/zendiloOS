//! Whether registries publish newer images than those the containers run,
//! and taking them on a schedule.

use zendiloos_kernel::Timestamp;

/// An image by registry, repository and tag, as a registry is asked about
/// it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryImage {
    /// The registry's host: `registry-1.docker.io`, `ghcr.io`.
    pub registry: String,
    /// `library/postgres`, `owner/app`.
    pub repository: String,
    pub tag: String,
}

/// Docker Hub, as images without a registry name it.
const DOCKER_HUB: &str = "registry-1.docker.io";

impl RegistryImage {
    /// `postgres:17`, `ghcr.io/owner/app`, `localhost:5000/app:1`; `None`
    /// for an image pinned by digest, which has nothing newer.
    pub fn parse(reference: &str) -> Option<Self> {
        if reference.contains('@') || reference.is_empty() {
            return None;
        }
        let (name, tag) = match reference.rsplit_once(':') {
            Some((name, tag)) if !tag.contains('/') => (name, tag),
            _ => (reference, "latest"),
        };
        let (first, rest) = name.split_once('/').unwrap_or(("", name));
        let is_registry = first.contains('.') || first.contains(':') || first == "localhost";
        let (registry, repository) = match (first, is_registry) {
            ("", _) => (DOCKER_HUB.to_owned(), format!("library/{rest}")),
            (registry, true) => {
                let registry = if registry == "docker.io" { DOCKER_HUB } else { registry };
                let repository = if registry == DOCKER_HUB && !rest.contains('/') {
                    format!("library/{rest}")
                } else {
                    rest.to_owned()
                };
                (registry.to_owned(), repository)
            }
            (_, false) => (DOCKER_HUB.to_owned(), name.to_owned()),
        };
        Some(Self { registry, repository, tag: tag.to_owned() })
    }

    /// The repository as Docker names it in an image's digests:
    /// `postgres`, `ghcr.io/owner/app`.
    pub fn local_repository(&self) -> String {
        if self.registry == DOCKER_HUB {
            self.repository.strip_prefix("library/").unwrap_or(&self.repository).to_owned()
        } else {
            format!("{}/{}", self.registry, self.repository)
        }
    }
}

/// Days of the week and a time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSchedule {
    /// Monday first.
    pub days: [bool; 7],
    /// Minutes after midnight, in the machine's time zone.
    pub minute: u16,
}

impl UpdateSchedule {
    /// Whether it is time: `weekday` 0 is Monday.
    pub fn due(&self, weekday: usize, minute: u16) -> bool {
        self.days.get(weekday).copied().unwrap_or(false) && minute == self.minute
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSettings {
    /// Registries are asked every few hours.
    pub check: bool,
    /// Newer images are taken on this schedule; `None`: by hand only.
    pub schedule: Option<UpdateSchedule>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check: true, schedule: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidUpdateSettings {
    #[error("a time of day is from 00:00 to 23:59")]
    Minute,
    #[error("a schedule needs at least one day")]
    NoDay,
    #[error("updating on a schedule needs the check")]
    NoCheck,
}

impl UpdateSettings {
    pub fn checked(self) -> Result<Self, InvalidUpdateSettings> {
        if let Some(schedule) = self.schedule {
            if schedule.minute >= 24 * 60 {
                return Err(InvalidUpdateSettings::Minute);
            }
            if !schedule.days.contains(&true) {
                return Err(InvalidUpdateSettings::NoDay);
            }
            if !self.check {
                return Err(InvalidUpdateSettings::NoCheck);
            }
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Current,
    /// The registry has another image under the tag.
    Available,
    /// Why it cannot be told: no registry, an image built here, an error.
    Unknown(String),
}

/// One image the containers run, as checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCheck {
    /// As the containers name it: `postgres:17`.
    pub reference: String,
    pub state: UpdateState,
    /// The containers made from it, by name.
    pub containers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateReport {
    pub checked_at: Timestamp,
    pub images: Vec<ImageCheck>,
}

impl UpdateReport {
    /// The references with a newer image.
    pub fn available(&self) -> impl Iterator<Item = &ImageCheck> {
        self.images.iter().filter(|image| image.state == UpdateState::Available)
    }
}

/// Whether `remote`, the registry's digest, is none of `local`, the
/// image's digests as Docker keeps them (`postgres@sha256:…`).
pub fn is_newer(image: &RegistryImage, remote: &str, local: &[String]) -> UpdateState {
    let repository = image.local_repository();
    let known: Vec<&str> = local
        .iter()
        .filter_map(|digest| digest.split_once('@'))
        .filter(|(repo, _)| *repo == repository)
        .map(|(_, digest)| digest)
        .collect();
    if known.is_empty() {
        UpdateState::Unknown("the image was not pulled from this registry".into())
    } else if known.contains(&remote) {
        UpdateState::Current
    } else {
        UpdateState::Available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(registry: &str, repository: &str, tag: &str) -> Option<RegistryImage> {
        Some(RegistryImage { registry: registry.into(), repository: repository.into(), tag: tag.into() })
    }

    #[test]
    fn references_name_their_registry_and_repository() {
        assert_eq!(RegistryImage::parse("postgres:17"), image(DOCKER_HUB, "library/postgres", "17"));
        assert_eq!(RegistryImage::parse("redis"), image(DOCKER_HUB, "library/redis", "latest"));
        assert_eq!(
            RegistryImage::parse("teslamate/teslamate:latest"),
            image(DOCKER_HUB, "teslamate/teslamate", "latest")
        );
        assert_eq!(
            RegistryImage::parse("docker.io/library/nginx:1.27"),
            image(DOCKER_HUB, "library/nginx", "1.27")
        );
        assert_eq!(RegistryImage::parse("docker.io/nginx"), image(DOCKER_HUB, "library/nginx", "latest"));
        assert_eq!(
            RegistryImage::parse("ghcr.io/maintainerr/maintainerr:latest"),
            image("ghcr.io", "maintainerr/maintainerr", "latest")
        );
        assert_eq!(RegistryImage::parse("localhost:5000/app"), image("localhost:5000", "app", "latest"));
        assert_eq!(
            RegistryImage::parse("lscr.io/linuxserver/sonarr:4"),
            image("lscr.io", "linuxserver/sonarr", "4")
        );
        assert_eq!(RegistryImage::parse("louislam/uptime-kuma@sha256:e884"), None, "pinned");
        assert_eq!(RegistryImage::parse("postgres:17").unwrap().local_repository(), "postgres");
        assert_eq!(RegistryImage::parse("ghcr.io/a/b").unwrap().local_repository(), "ghcr.io/a/b");
    }

    #[test]
    fn newer_means_a_digest_the_image_does_not_have() {
        let postgres = RegistryImage::parse("postgres:17").unwrap();
        let local = vec!["postgres@sha256:aaa".to_owned(), "other@sha256:bbb".to_owned()];
        assert_eq!(is_newer(&postgres, "sha256:aaa", &local), UpdateState::Current);
        assert_eq!(is_newer(&postgres, "sha256:bbb", &local), UpdateState::Available, "another repository's");
        assert!(matches!(is_newer(&postgres, "sha256:aaa", &[]), UpdateState::Unknown(_)));
    }

    #[test]
    fn schedules_are_checked_and_come_due_once_a_minute() {
        let schedule = UpdateSchedule { days: [true, false, false, false, false, false, true], minute: 340 };
        assert!(schedule.due(0, 340) && schedule.due(6, 340));
        assert!(!schedule.due(1, 340) && !schedule.due(0, 341));
        let settings = UpdateSettings { check: true, schedule: Some(schedule) };
        assert_eq!(settings.checked(), Ok(settings));
        let late = UpdateSettings { schedule: Some(UpdateSchedule { minute: 1440, ..schedule }), ..settings };
        assert_eq!(late.checked(), Err(InvalidUpdateSettings::Minute));
        let never =
            UpdateSettings { schedule: Some(UpdateSchedule { days: [false; 7], ..schedule }), ..settings };
        assert_eq!(never.checked(), Err(InvalidUpdateSettings::NoDay));
        assert_eq!(
            UpdateSettings { check: false, ..settings }.checked(),
            Err(InvalidUpdateSettings::NoCheck)
        );
    }
}
