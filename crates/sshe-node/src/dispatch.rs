use crate::{Error, Result, config::Config, history::History, probe::probe};
use iroh::Endpoint;
use sshe_protocol::{Health, HistoryReport, ProbeReport, Request, Response, VERSION};
use tokio::sync::RwLock;
/// What a request may use on this side; absent parts make dependent requests fail.
pub(crate) struct Node<'a> {
    pub(crate) config: &'a Config,
    pub(crate) id: String,
    pub(crate) endpoint: Option<&'a Endpoint>,
    pub(crate) history: Option<&'a RwLock<History>>,
}

pub(crate) async fn dispatch(node: &Node<'_>, request: Request) -> Result<Response> {
    Ok(match request {
        Request::Health => Response::Health(Health {
            endpoint_id: node.id.clone(),
            status: "ok".into(),
            version: VERSION.into(),
        }),
        Request::Probe { kind } => Response::Probe(ProbeReport {
            observer: node.id.clone(),
            kind,
            records: probe(node.config, node.endpoint, kind).await?,
        }),
        Request::History { kind, about, limit } => {
            let history = node
                .history
                .ok_or_else(|| Error::Invalid("history requires a running daemon".into()))?;
            Response::History(HistoryReport {
                observer: node.id.clone(),
                queried_at: sshe_core::now(),
                records: history.read().await.query(kind, about.as_deref(), limit),
            })
        }
        Request::Exec {
            program,
            args,
            timeout_secs,
        } => Response::Exec(sshe_core::execute(&program, &args, timeout_secs).await?),
    })
}
pub(crate) fn response(result: Result<Response>) -> Response {
    result.unwrap_or_else(|e| Response::Error(e.to_string()))
}
