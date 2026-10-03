//! One sampling round per kind, shared by manual requests and scheduled probes.
use crate::{
    Error, Result,
    config::{Config, DaemonConfig},
    history::History,
    probe::probe,
    transport::Dialer,
};
use sshe_protocol::{ProbeKind, Record};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{RwLock, watch},
    time::Instant,
};

mod log;

// A round has its own channel, so later rounds cannot replace a waiter's result.
type SharedResult = std::result::Result<Vec<Record>, Arc<Error>>;
type Round = watch::Receiver<Option<SharedResult>>;

#[derive(Default)]
struct Slot {
    running: Mutex<Option<Round>>,
    next: watch::Sender<Option<Instant>>,
}

pub(crate) struct Sampler {
    slots: BTreeMap<ProbeKind, Slot>,
    interval: Duration,
    pub(crate) history: RwLock<History>,
}

impl Sampler {
    pub(crate) fn new(settings: &DaemonConfig) -> Self {
        Self {
            slots: ProbeKind::ALL
                .into_iter()
                .map(|kind| (kind, Slot::default()))
                .collect(),
            interval: Duration::from_secs(settings.interval_secs),
            history: RwLock::new(History::new(settings.history_size)),
        }
    }

    pub(crate) fn deadline(&self, kind: ProbeKind) -> watch::Receiver<Option<Instant>> {
        self.slots[&kind].next.subscribe()
    }

    pub(crate) async fn run(
        &self,
        config: &Config,
        dialer: &Dialer,
        kind: ProbeKind,
    ) -> Result<Vec<Record>> {
        self.sample(kind, false, probe(config, Some(dialer), kind))
            .await
    }

    pub(crate) async fn run_due(
        &self,
        config: &Config,
        dialer: &Dialer,
        kind: ProbeKind,
    ) -> Result<()> {
        self.sample(kind, true, probe(config, Some(dialer), kind))
            .await
            .map(|_| ())
    }

    async fn sample(
        &self,
        kind: ProbeKind,
        due_only: bool,
        work: impl Future<Output = Result<Vec<Record>>>,
    ) -> Result<Vec<Record>> {
        let slot = &self.slots[&kind];
        let (mut receiver, leader) = {
            let mut running = slot.running.lock().expect("probe slot poisoned");
            if let Some(round) = running.as_ref().filter(|r| r.has_changed().is_ok()) {
                (round.clone(), None)
            } else {
                if due_only && slot.next.borrow().is_some_and(|due| due > Instant::now()) {
                    return Ok(Vec::new());
                }
                let (sender, receiver) = watch::channel(None);
                *running = Some(receiver.clone());
                (receiver, Some(sender))
            }
        };
        if let Some(sender) = leader {
            let mut result = work.await;
            if let Ok(records) = &mut result {
                let mut history = self.history.write().await;
                for record in records {
                    let transition = history.insert(record);
                    log::log(record, transition);
                }
            }
            let result = result.map_err(Arc::new);
            // Publish only after history and the completion-based deadline agree.
            let mut running = slot.running.lock().expect("probe slot poisoned");
            slot.next.send_replace(Some(Instant::now() + self.interval));
            sender.send_replace(Some(result.clone()));
            *running = None;
            return result.map_err(Error::ProbeFailed);
        }
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result.map_err(Error::ProbeFailed);
            }
            // Dropping the leader closes this round; waiters do not hang, and
            // the next caller can start a fresh round instead of using stale state.
            receiver
                .changed()
                .await
                .map_err(|_| Error::ProbeCancelled)?;
        }
    }
}

#[cfg(test)]
mod tests;
