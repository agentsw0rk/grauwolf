//! Collects destinations, merging duplicates.

use std::collections::{BTreeMap, BTreeSet};

use crate::target::{self, Target};
use crate::{Destination, Phase};

type Key = (
    Phase,
    Option<String>,
    Option<String>,
    Option<u16>,
    bool,
    Option<String>,
);

#[derive(Default)]
pub struct Collector {
    entries: BTreeMap<Key, (BTreeSet<String>, BTreeSet<String>)>,
}

impl Collector {
    pub fn add(&mut self, key: Key, purpose: &str, pointer: &str) {
        let entry = self.entries.entry(key).or_default();
        entry.0.insert(purpose.to_owned());
        entry.1.insert(pointer.to_owned());
    }

    /// An address given as a URL (or `host:port`); one that does not parse is not a destination
    /// (validation reports it).
    pub fn url(
        &mut self,
        phase: Phase,
        text: &str,
        purpose: &str,
        pointer: &str,
        from_env: Option<&str>,
    ) {
        if let Some(Target { scheme, host, port }) = target::parse(text, "https") {
            self.add(
                (
                    phase,
                    Some(scheme),
                    Some(host),
                    port,
                    false,
                    from_env.map(str::to_owned),
                ),
                purpose,
                pointer,
            );
        }
    }

    /// An address that only a variable knows.
    pub fn by_variable(
        &mut self,
        scheme: Option<&str>,
        variable: &str,
        purpose: &str,
        pointer: &str,
    ) {
        self.add(
            (
                Phase::Run,
                scheme.map(str::to_owned),
                None,
                None,
                false,
                Some(variable.to_owned()),
            ),
            purpose,
            pointer,
        );
    }

    pub fn host(
        &mut self,
        phase: Phase,
        scheme: &str,
        host: &str,
        port: u16,
        purpose: &str,
        pointer: &str,
    ) {
        self.add(
            (
                phase,
                Some(scheme.to_owned()),
                Some(host.to_ascii_lowercase()),
                Some(port),
                false,
                None,
            ),
            purpose,
            pointer,
        );
    }

    /// A host of the cluster whose port the spec does not name: any port of it.
    pub fn host_any_port(
        &mut self,
        phase: Phase,
        scheme: &str,
        host: &str,
        purpose: &str,
        pointer: &str,
    ) {
        self.add(
            (
                phase,
                Some(scheme.to_owned()),
                Some(host.to_ascii_lowercase()),
                None,
                true,
                None,
            ),
            purpose,
            pointer,
        );
    }

    pub fn finish(self) -> Vec<Destination> {
        self.entries
            .into_iter()
            .map(
                |((phase, scheme, host, port, any_port, from_env), (purposes, pointers))| {
                    Destination {
                        scheme,
                        host,
                        port,
                        any_port,
                        purpose: purposes.into_iter().collect::<Vec<_>>().join("; "),
                        pointer: pointers.into_iter().collect::<Vec<_>>().join(", "),
                        phase,
                        from_env,
                    }
                },
            )
            .collect()
    }
}

impl Collector {
    /// Adds already merged destinations again (their purposes and pointers are split first).
    pub fn extend(&mut self, destinations: impl IntoIterator<Item = Destination>) {
        for destination in destinations {
            let key = (
                destination.phase,
                destination.scheme,
                destination.host,
                destination.port,
                destination.any_port,
                destination.from_env,
            );
            let entry = self.entries.entry(key).or_default();
            entry
                .0
                .extend(destination.purpose.split("; ").map(str::to_owned));
            entry
                .1
                .extend(destination.pointer.split(", ").map(str::to_owned));
        }
    }
}
