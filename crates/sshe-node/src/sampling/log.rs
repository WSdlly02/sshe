use crate::history::Transition;
use sshe_protocol::Record;

pub(super) fn log(record: &Record, transition: Transition) {
    let kind = record.kind.as_str();
    let target = record.label.as_deref().unwrap_or(&record.target);
    let method = record.method.as_str();
    let error = record.error.as_deref().unwrap_or_default();
    let error_kind = record.error_kind.map(|k| k.as_str()).unwrap_or_default();
    match (transition, record.success) {
        (Transition::First, true) => {
            tracing::info!(kind, target, method, ms = record.duration_ms, "check ok")
        }
        (Transition::First | Transition::Failed, false) => {
            tracing::warn!(kind, target, method, error_kind, error, "check failed")
        }
        // Failures are only noticed a check later, so time since the last success
        // bounds the outage from above; counting from the first failure undercounts.
        (Transition::Recovered { last_success_at }, _) => tracing::info!(
            kind,
            target,
            method,
            since_last_ok_secs = last_success_at.map(|at| record.observed_at.saturating_sub(at)),
            "check recovered"
        ),
        _ => tracing::debug!(
            kind,
            target,
            method,
            success = record.success,
            error,
            "check"
        ),
    }
}
