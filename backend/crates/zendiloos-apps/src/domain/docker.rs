//! Docker on the NAS: installed from Debian, its data on a pool.

/// A pool zendiloos created, where Docker's data can go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppVolume {
    pub pool: String,
    pub number: u16,
}

/// Docker as the host has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerHostState {
    pub installed: bool,
    pub running: bool,
    pub data_root: Option<String>,
}

/// The volume number in a data root zendiloOS set: `/volume<N>/@docker`.
pub fn docker_volume(data_root: &str) -> Option<u16> {
    data_root.strip_prefix("/volume")?.strip_suffix("/@docker")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_data_roots_zendiloos_set_count() {
        assert_eq!(docker_volume("/volume2/@docker"), Some(2));
        assert_eq!(docker_volume("/var/lib/docker"), None);
        assert_eq!(docker_volume("/volume2/@docker/x"), None);
    }
}
