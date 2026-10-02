use anyhow::{Result, bail};
use sshe_protocol::Response;
use std::io::Write;

pub(crate) fn render(response: Response, json: bool) -> Result<u8> {
    match response {
        Response::Probe(report) => println!("{}", serde_json::to_string_pretty(&report)?),
        Response::History(report) => println!("{}", serde_json::to_string_pretty(&report)?),
        Response::Health(_) => bail!("unexpected health response"),
        Response::Error(message) => bail!("{message}"),
        Response::Exec(result) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                std::io::stdout().write_all(&result.stdout)?;
                std::io::stderr().write_all(&result.stderr)?;
            }
            return Ok(result
                .exit_code
                .unwrap_or_else(|| 128 + result.signal.unwrap_or(1))
                .clamp(0, 255) as u8);
        }
    }
    Ok(0)
}
