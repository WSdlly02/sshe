use crate::now;
use serde_json::Value;
use sshe_protocol::{ProbeKind, Record};
use std::{
    fmt::Display,
    time::{Duration, Instant},
};
use tokio::time::timeout;
/// Runs one bounded check; failure and timeout become part of the record.
pub async fn measure<E: Display>(
    kind: ProbeKind,
    target: String,
    label: Option<String>,
    method: &str,
    limit: Duration,
    f: impl Future<Output = std::result::Result<Option<Value>, E>>,
) -> Record {
    let start = Instant::now();
    let (error, data) = match timeout(limit, f).await {
        Ok(Ok(data)) => (None, data),
        Ok(Err(e)) => (Some(format!("{e:#}")), None),
        Err(_) => (Some("timeout".into()), None),
    };
    let at = now();
    Record {
        kind,
        target,
        label,
        method: method.into(),
        observed_at: at,
        duration_ms: start.elapsed().as_millis() as u64,
        success: error.is_none(),
        last_success_at: error.is_none().then_some(at),
        error,
        data,
    }
}
