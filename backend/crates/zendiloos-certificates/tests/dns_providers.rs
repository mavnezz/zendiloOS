//! Every DNS provider offered is a script of acme.sh as Debian 13 ships it,
//! and every value it is given is one that script reads: a misspelt name
//! would reach acme.sh and be ignored there, and the challenge would fail
//! with nothing said why. `acme-sh-dnsapi.txt` comes from Debian's sources
//! (`scripts/acme-dnsapi-names.sh 3.1.1-1 …`).

use std::collections::{BTreeMap, BTreeSet};

use zendiloos_certificates::domain::DNS_PROVIDERS;

const READ_BY_SCRIPTS: &str = include_str!("acme-sh-dnsapi.txt");

fn scripts() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    READ_BY_SCRIPTS
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((words.next()?, words.collect()))
        })
        .collect()
}

#[test]
fn every_provider_is_a_script_that_reads_the_values_it_is_given() {
    let scripts = scripts();
    for provider in DNS_PROVIDERS {
        let read =
            scripts.get(provider.api).unwrap_or_else(|| panic!("{} is no script of acme.sh", provider.api));
        assert!(!provider.variables.is_empty(), "{} takes no value", provider.api);
        for variable in provider.variables {
            assert!(read.contains(variable.name), "{} does not read {}", provider.api, variable.name);
        }
    }
}

#[test]
fn providers_and_their_values_are_named_once() {
    let mut apis = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for provider in DNS_PROVIDERS {
        assert!(apis.insert(provider.api), "{} twice", provider.api);
        assert!(labels.insert(provider.label), "{} twice", provider.label);
        let mut names = BTreeSet::new();
        for variable in provider.variables {
            assert!(names.insert(variable.name), "{} has {} twice", provider.api, variable.name);
        }
        assert!(provider.variables.iter().any(|v| v.secret), "{} has no secret value", provider.api);
    }
}
