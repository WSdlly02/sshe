use crate::{Error, Result, config::Config, probe::probe, sampling::Sampler, transport::Dialer};
use sshe_protocol::{Health, HistoryReport, ProbeReport, Request, Response, VERSION};
/// What a request may use on this side; absent parts make dependent requests fail.
pub(crate) struct Node<'a> {
    pub(crate) config: &'a Config,
    pub(crate) id: String,
    pub(crate) dialer: Option<&'a Dialer>,
    pub(crate) sampler: Option<&'a Sampler>,
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
            records: match (node.sampler, node.dialer) {
                (Some(sampler), Some(dialer)) => sampler.run(node.config, dialer, kind).await?,
                _ => probe(node.config, node.dialer, kind).await?,
            },
        }),
        Request::History { kind, about, limit } => {
            let sampler = node
                .sampler
                .ok_or_else(|| Error::Invalid("history requires a running daemon".into()))?;
            Response::History(HistoryReport {
                observer: node.id.clone(),
                queried_at: sshe_core::now(),
                records: sampler
                    .history
                    .read()
                    .await
                    .query(kind, about.as_deref(), limit),
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
