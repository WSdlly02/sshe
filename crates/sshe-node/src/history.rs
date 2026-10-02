use sshe_protocol::{MAX_HISTORY_QUERY, ProbeKind, Record};
use std::collections::{BTreeMap, VecDeque};

/// In-memory probe results, bounded per series; cleared on restart.
pub struct History {
    capacity: usize,
    series: BTreeMap<(ProbeKind, String, String), Series>,
}

#[derive(Default)]
struct Series {
    records: VecDeque<Record>,
    /// Survives eviction so long outages still report when the target last worked.
    last_success_at: Option<u64>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            series: BTreeMap::new(),
        }
    }
    pub fn insert(&mut self, mut record: Record) {
        let key = (record.kind, record.method.clone(), record.target.clone());
        let series = self.series.entry(key).or_default();
        if record.success {
            series.last_success_at = Some(record.observed_at);
        }
        record.last_success_at = series.last_success_at;
        if series.records.len() == self.capacity {
            series.records.pop_front();
        }
        series.records.push_back(record);
    }
    /// Most recent `limit` matching records, oldest first.
    pub fn query(&self, kind: Option<ProbeKind>, about: Option<&str>, limit: usize) -> Vec<Record> {
        let mut records: Vec<Record> = self
            .series
            .iter()
            .filter(|((k, _, _), _)| kind.is_none_or(|kind| kind == *k))
            .flat_map(|(_, series)| &series.records)
            .filter(|r| about.is_none_or(|a| r.target == a || r.label.as_deref() == Some(a)))
            .cloned()
            .collect();
        records.sort_by_key(|r| r.observed_at);
        let keep = limit.min(MAX_HISTORY_QUERY);
        records.split_off(records.len().saturating_sub(keep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(target: &str, at: u64, success: bool) -> Record {
        Record {
            kind: ProbeKind::Peers,
            target: target.into(),
            label: Some(format!("{target}-alias")),
            method: "iroh_health".into(),
            observed_at: at,
            duration_ms: 1,
            success,
            error: (!success).then(|| "down".into()),
            last_success_at: success.then_some(at),
            data: None,
        }
    }
    #[test]
    fn bounded_per_series_and_keeps_last_success_after_eviction() {
        let mut history = History::new(2);
        history.insert(record("a", 1, true));
        history.insert(record("a", 2, false));
        history.insert(record("a", 3, false));
        history.insert(record("b", 4, true));
        let a = history.query(None, Some("a"), 10);
        assert_eq!(a.iter().map(|r| r.observed_at).collect::<Vec<_>>(), [2, 3]);
        assert!(a.iter().all(|r| r.last_success_at == Some(1)));
        assert_eq!(history.query(None, Some("b-alias"), 10).len(), 1);
        assert_eq!(history.query(Some(ProbeKind::Wan), None, 10).len(), 0);
        let latest = history.query(None, None, 2);
        assert_eq!(
            latest.iter().map(|r| r.observed_at).collect::<Vec<_>>(),
            [3, 4]
        );
    }
}
