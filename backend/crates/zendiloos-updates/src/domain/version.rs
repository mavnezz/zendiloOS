//! Debian's version numbers, compared as dpkg compares them.

use std::cmp::Ordering;

/// `a` against `b` as `dpkg --compare-versions` has it: the epoch, then
/// the upstream version, then the revision; `~` sorts before everything,
/// even the end (`1.0~rc1` < `1.0` < `1.0+dev1`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (epoch_a, upstream_a, revision_a) = split(a);
    let (epoch_b, upstream_b, revision_b) = split(b);
    epoch_a
        .cmp(&epoch_b)
        .then_with(|| compare_part(upstream_a.as_bytes(), upstream_b.as_bytes()))
        .then_with(|| compare_part(revision_a.as_bytes(), revision_b.as_bytes()))
}

/// The epoch (0 if none), the upstream version and the revision.
fn split(version: &str) -> (u64, &str, &str) {
    let (epoch, rest) = match version.split_once(':') {
        Some((epoch, rest)) if epoch.bytes().all(|b| b.is_ascii_digit()) => {
            (epoch.parse().unwrap_or(0), rest)
        }
        _ => (0, version),
    };
    match rest.rsplit_once('-') {
        Some((upstream, revision)) => (epoch, upstream, revision),
        None => (epoch, rest, ""),
    }
}

/// dpkg's weight of a character in the non-digit runs.
fn order(c: Option<u8>) -> i32 {
    match c {
        None => 0,
        Some(c) if c.is_ascii_digit() => 0,
        Some(c) if c.is_ascii_alphabetic() => i32::from(c),
        Some(b'~') => -1,
        Some(c) => i32::from(c) + 256,
    }
}

fn compare_part(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let (x, y) = (order(a.get(i).copied()), order(b.get(j).copied()));
            if x != y {
                return x.cmp(&y);
            }
            i += 1;
            j += 1;
        }
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        let mut first = Ordering::Equal;
        while i < a.len() && a[i].is_ascii_digit() && j < b.len() && b[j].is_ascii_digit() {
            if first == Ordering::Equal {
                first = a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
        if i < a.len() && a[i].is_ascii_digit() {
            return Ordering::Greater;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            return Ordering::Less;
        }
        if first != Ordering::Equal {
            return first;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_sort_as_dpkg_sorts_them() {
        let ascending = [
            "0.1.0",
            "0.2.0~dev.1",
            "0.2.0~rc.1",
            "0.2.0~rc.2",
            "0.2.0",
            "0.2.0+dev.20261106120000.gabc1234",
            "0.2.0+dev.20261107080000.gdef5678",
            "0.2.1",
            "0.10.0",
            "1:0.0.1",
        ];
        for pair in ascending.windows(2) {
            assert_eq!(compare_versions(pair[0], pair[1]), Ordering::Less, "{} < {}", pair[0], pair[1]);
            assert_eq!(compare_versions(pair[1], pair[0]), Ordering::Greater, "{} > {}", pair[1], pair[0]);
        }
        assert_eq!(compare_versions("1.0", "1.00"), Ordering::Equal);
        assert_eq!(compare_versions("2.40-1", "2.40-1"), Ordering::Equal);
        assert_eq!(compare_versions("2.40-1", "2.40-1+deb13u1"), Ordering::Less);
        assert_eq!(compare_versions("6.12.48+deb13-amd64", "6.12.57+deb13-amd64"), Ordering::Less);
        assert_eq!(compare_versions("1.0~", "1.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0a", "1.0+"), Ordering::Less);
    }
}
