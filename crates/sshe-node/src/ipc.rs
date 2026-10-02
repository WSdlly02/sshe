use serde::{Deserialize, Serialize};
use sshe_protocol::Request;

#[derive(Serialize, Deserialize)]
pub(crate) struct LocalRequest {
    pub(crate) target: Option<String>,
    pub(crate) request: Request,
}
