//! Runs the configured probes each interval and records results; never serves requests.
use crate::{dispatch::Node, history::History, probe::probe};
use futures::future::join_all;
use sshe_protocol::ProbeKind;
use std::{collections::BTreeSet, time::Duration};
use tokio::sync::RwLock;

pub(crate) async fn run(node: &Node<'_>, history: &RwLock<History>) {
    let settings = &node.config.daemon;
    let kinds: BTreeSet<ProbeKind> = settings.probes.iter().copied().collect();
    let mut interval = tokio::time::interval(Duration::from_secs(settings.interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let rounds = join_all(
            kinds
                .iter()
                .map(|&kind| probe(node.config, node.endpoint, kind)),
        )
        .await;
        let mut history = history.write().await;
        for round in rounds {
            match round {
                Ok(records) => records.into_iter().for_each(|r| history.insert(r)),
                Err(error) => eprintln!("scheduled probe failed: {error}"),
            }
        }
    }
}
