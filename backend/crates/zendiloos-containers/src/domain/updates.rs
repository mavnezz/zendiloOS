//! Whether registries publish newer images than those the containers run,
//! and taking them on a schedule.

use zendiloos_kernel::Timestamp;

use super::calendar::{UtcOffsets, date, day_number, local, moment, weekday};

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

/// Days of the week and a time of day. Due as the backup plans are: once
/// the latest of its times (the host's local time, daylight saving time
/// included) lies after it was set or last ran, so a minute missed (the
/// NAS off, asleep or busy, its clock changed) is made up once, as soon as
/// it can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSchedule {
    /// Monday first.
    pub days: [bool; 7],
    /// Minutes after midnight, in the machine's time zone.
    pub minute: u16,
}

impl UpdateSchedule {
    /// Its time on the local date `days` after 1 January 1970, if the date
    /// is one of its days.
    fn on_day(&self, days: i64, offsets: UtcOffsets<'_>) -> Option<Timestamp> {
        let date = date(days);
        let chosen = usize::try_from(weekday(date)).ok().and_then(|day| self.days.get(day).copied());
        let (hour, minute) = (u8::try_from(self.minute / 60).ok()?, u8::try_from(self.minute % 60).ok()?);
        chosen.unwrap_or(false).then(|| moment(date.year, date.month, date.day, hour, minute, offsets))
    }

    /// The latest of its times at or before `now`; none without a day.
    pub fn latest(&self, now: Timestamp, offsets: UtcOffsets<'_>) -> Option<Timestamp> {
        let today = day_number(local(now, offsets(now)));
        (0..=7).filter_map(|back| self.on_day(today - back, offsets)).find(|at| *at <= now)
    }

    /// Whether it should update at `now`, having been set or run at
    /// `since`.
    pub fn is_due(&self, now: Timestamp, since: Timestamp, offsets: UtcOffsets<'_>) -> bool {
        self.latest(now, offsets).is_some_and(|at| at > since)
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

    /// 2026-11-02 00:00:00 UTC, a Monday.
    const MONDAY: i64 = 1_793_577_600;
    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(MONDAY + seconds)
    }

    #[test]
    fn a_schedule_is_due_from_its_latest_time_after_it_was_set_or_ran() {
        // Mondays and Sundays at 05:40, UTC.
        let schedule = UpdateSchedule { days: [true, false, false, false, false, false, true], minute: 340 };
        let utc = &|_: Timestamp| 0;
        let monday = at(340 * 60);
        assert_eq!(schedule.latest(monday, utc), Some(monday));
        assert_eq!(schedule.latest(at(340 * 60 - 1), utc), Some(at(340 * 60 - DAY)), "Sunday's, before");
        assert_eq!(schedule.latest(at(3 * DAY), utc), Some(monday), "Thursday: Monday's");
        assert!(schedule.is_due(monday, at(0), utc));
        assert!(!schedule.is_due(monday, monday, utc), "ran then");
        // Missed (off, asleep, busy): due on the next look, hours later.
        assert!(schedule.is_due(at(2 * DAY), at(0), utc));
        assert!(!schedule.is_due(at(2 * DAY), at(2 * DAY - HOUR), utc), "once");
        let never = UpdateSchedule { days: [false; 7], ..schedule };
        assert_eq!(never.latest(monday, utc), None);
    }

    #[test]
    fn a_schedule_keeps_to_the_local_time_across_summer_time() {
        // Berlin: summer time ends on 2026-10-25 at 03:00 (01:00 UTC), and
        // begins on 2027-03-28 at 02:00 (01:00 UTC).
        let berlin = &|at: Timestamp| {
            let seconds = at.unix_seconds();
            if (1_792_890_000..1_806_195_600).contains(&seconds) { 3600 } else { 7200 }
        };
        let daily = UpdateSchedule { days: [true; 7], minute: 150 };
        let utc = Timestamp::from_unix_seconds;
        // Sunday 25 October shows 02:30 twice: the first counts, once; on
        // Monday it is 02:30 winter time, an hour later in UTC.
        let sunday = utc(1_792_888_200);
        assert_eq!(daily.latest(utc(1_792_889_100), berlin), Some(sunday));
        assert_eq!(daily.latest(utc(1_792_892_700), berlin), Some(sunday), "not again an hour later");
        assert_eq!(daily.latest(utc(1_792_980_000), berlin), Some(utc(1_792_978_200)));
        // Sunday 28 March skips 02:30: 03:30 summer time instead.
        assert_eq!(daily.latest(utc(1_806_196_500), berlin), Some(utc(1_806_111_000)), "Saturday's still");
        assert_eq!(daily.latest(utc(1_806_199_200), berlin), Some(utc(1_806_197_400)));
    }

    #[test]
    fn schedules_are_checked() {
        let schedule = UpdateSchedule { days: [true, false, false, false, false, false, true], minute: 340 };
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
