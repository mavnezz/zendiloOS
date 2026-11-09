//! Who keeps which containers stopped while volumes are backed up and
//! restored (docs/architecture.md, 4.9).
//!
//! Each backup or restore claims its volume when it is asked for, with the
//! containers it will keep stopped: those that use the volume and run, and
//! those another backup or restore stopped. They take turns, one at a time
//! on the whole NAS, and stop the containers only once their turn came. A
//! container stopped for one starts again only once the last claim holding
//! it ended, so a container of two volumes backed up one after the other
//! stays stopped until both are done.

use std::collections::{BTreeMap, BTreeSet};

/// Why a volume cannot be claimed now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unclaimable {
    #[error("a backup or restore of the volume {0} is under way")]
    Busy(String),
    #[error("Docker's data is being moved")]
    Moving,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Claim {
    /// What it keeps stopped while it lasts.
    containers: BTreeSet<String>,
    /// The volume's containers must not run while it works: a restore, or
    /// a backup that stops them.
    stops: bool,
}

/// The volumes claimed for backups and restores, whose turn it is, and the
/// containers they keep stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holds {
    claims: BTreeMap<String, Claim>,
    /// Claimed volumes waiting for their turn, in the order they came.
    queue: Vec<String>,
    /// The volume whose turn it is.
    running: Option<String>,
    /// Containers a backup or restore stopped, not started again since.
    stopped: BTreeSet<String>,
    /// Docker's data is being moved.
    moving: bool,
}

impl Holds {
    /// Claims `volume`, holding `containers`; refused while it is claimed
    /// already or Docker's data moves.
    pub fn claim(
        &mut self,
        volume: &str,
        stops: bool,
        containers: impl IntoIterator<Item = String>,
    ) -> Result<(), Unclaimable> {
        if self.moving {
            return Err(Unclaimable::Moving);
        }
        if self.claims.contains_key(volume) {
            return Err(Unclaimable::Busy(volume.to_owned()));
        }
        self.claims.insert(volume.to_owned(), Claim { containers: containers.into_iter().collect(), stops });
        self.queue.push(volume.to_owned());
        Ok(())
    }

    pub fn is_claimed(&self, volume: &str) -> bool {
        self.claims.contains_key(volume)
    }

    /// A claimed volume, the one whose turn it is first.
    pub fn any(&self) -> Option<&str> {
        self.running.as_deref().or_else(|| self.claims.keys().next().map(String::as_str))
    }

    /// Whether a claim holds `container`.
    pub fn is_held(&self, container: &str) -> bool {
        self.claims.values().any(|claim| claim.containers.contains(container))
    }

    /// Whether a backup or restore stopped `container` and nothing started
    /// it since.
    pub fn is_stopped(&self, container: &str) -> bool {
        self.stopped.contains(container)
    }

    /// The volume whose turn is next, if none runs: one that holds a
    /// container another stopped first, so that it is stopped no longer
    /// than need be; otherwise the one claimed first.
    pub fn next(&self) -> Option<&str> {
        if self.running.is_some() {
            return None;
        }
        let waits_on_it = |volume: &&String| {
            self.claims[*volume].containers.iter().any(|container| self.stopped.contains(container))
        };
        self.queue.iter().find(waits_on_it).or_else(|| self.queue.first()).map(String::as_str)
    }

    /// Takes `volume`'s turn if it is next; whether it did.
    pub fn take_turn(&mut self, volume: &str) -> bool {
        if self.next() != Some(volume) {
            return false;
        }
        self.queue.retain(|queued| queued != volume);
        self.running = Some(volume.to_owned());
        true
    }

    /// `container` was stopped for `volume`'s claim, which holds it now.
    pub fn stopped(&mut self, volume: &str, container: &str) {
        if let Some(claim) = self.claims.get_mut(volume) {
            claim.containers.insert(container.to_owned());
        }
        self.stopped.insert(container.to_owned());
    }

    /// `container` runs again, or is gone.
    pub fn started(&mut self, container: &str) {
        self.stopped.remove(container);
    }

    /// `volume`'s claim ends: the containers it held that no other claim
    /// holds, to be started again if they were stopped.
    pub fn release(&mut self, volume: &str) -> Vec<String> {
        let Some(claim) = self.claims.remove(volume) else { return Vec::new() };
        self.queue.retain(|queued| queued != volume);
        if self.running.as_deref() == Some(volume) {
            self.running = None;
        }
        claim.containers.into_iter().filter(|container| !self.is_held(container)).collect()
    }

    /// Whether `container`, which mounts `volumes`, must be left alone now
    /// (not started, stopped, brought up anew or removed): a backup or
    /// restore keeps it stopped, or works on one of its volumes while their
    /// containers must not run.
    pub fn blocks(&self, container: &str, volumes: &[&str]) -> bool {
        let kept = self.stopped.contains(container) && self.is_held(container);
        let working = self.running.as_deref().is_some_and(|running| {
            self.claims.get(running).is_some_and(|claim| claim.stops) && volumes.contains(&running)
        });
        kept || working
    }

    /// Docker's data starts moving: refused while a volume is claimed.
    pub fn begin_move(&mut self) -> Result<(), Unclaimable> {
        if let Some(volume) = self.any() {
            return Err(Unclaimable::Busy(volume.to_owned()));
        }
        if self.moving {
            return Err(Unclaimable::Moving);
        }
        self.moving = true;
        Ok(())
    }

    pub fn end_move(&mut self) {
        self.moving = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_volume_is_claimed_once_and_they_take_turns_one_at_a_time() {
        let mut holds = Holds::default();
        holds.claim("db", true, names(&["app"])).unwrap();
        assert_eq!(holds.claim("db", false, []), Err(Unclaimable::Busy("db".into())));
        holds.claim("cache", false, []).unwrap();
        assert_eq!(holds.next(), Some("db"), "the first claimed");
        assert!(!holds.take_turn("cache"));
        assert!(holds.take_turn("db"));
        assert_eq!((holds.next(), holds.any()), (None, Some("db")), "one at a time");
        assert!(!holds.take_turn("cache"));
        assert!(holds.release("db") == ["app"] && holds.take_turn("cache"));
        assert!(holds.release("cache").is_empty() && holds.any().is_none());
        assert!(holds.release("never").is_empty());
    }

    #[test]
    fn a_container_stays_stopped_until_the_last_claim_holding_it_ended() {
        let mut holds = Holds::default();
        holds.claim("app_data", true, names(&["app"])).unwrap();
        assert!(holds.take_turn("app_data"));
        holds.stopped("app_data", "app");
        // Claimed meanwhile: it holds `app` too, which another stopped.
        holds.claim("other", false, []).unwrap();
        holds.claim("app_db", true, names(&["app"])).unwrap();
        assert!(holds.is_held("app") && holds.is_stopped("app"));
        assert!(holds.release("app_data").is_empty(), "app_db still holds it");
        assert_eq!(holds.next(), Some("app_db"), "before `other`: app waits on it");
        assert!(holds.take_turn("app_db"));
        assert_eq!(holds.release("app_db"), ["app"]);
        holds.started("app");
        assert!(!holds.is_stopped("app") && !holds.is_held("app"));
    }

    #[test]
    fn what_a_backup_or_restore_holds_is_left_alone() {
        let mut holds = Holds::default();
        holds.claim("db", true, names(&["web"])).unwrap();
        assert!(!holds.blocks("web", &["db"]), "not before its turn");
        assert!(holds.take_turn("db"));
        holds.stopped("db", "web");
        assert!(holds.blocks("web", &["db"]));
        // Stopped before by hand: it uses the volume all the same.
        assert!(holds.blocks("worker", &["db"]) && !holds.blocks("worker", &["other"]));
        holds.release("db");
        // Its start failed: it is owed one, but nothing holds it any more.
        assert!(!holds.blocks("web", &["db"]));
        holds.claim("logs", false, []).unwrap();
        assert!(holds.take_turn("logs"));
        assert!(!holds.blocks("worker", &["logs"]), "a backup that leaves them running");
    }

    #[test]
    fn dockers_data_moves_only_while_no_volume_is_claimed() {
        let mut holds = Holds::default();
        holds.claim("db", false, []).unwrap();
        assert_eq!(holds.begin_move(), Err(Unclaimable::Busy("db".into())));
        holds.release("db");
        holds.begin_move().unwrap();
        assert_eq!(holds.begin_move(), Err(Unclaimable::Moving));
        assert_eq!(holds.claim("db", false, []), Err(Unclaimable::Moving));
        holds.end_move();
        assert!(holds.claim("db", false, []).is_ok());
    }
}
