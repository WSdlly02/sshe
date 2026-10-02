use crate::{Error, Result};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use sshe_protocol::{ExecResult, MAX_EXEC_SECONDS, OUTPUT_LIMIT};
use std::{
    os::unix::process::ExitStatusExt,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0 as i32), Signal::SIGKILL);
    }
}

async fn read_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > OUTPUT_LIMIT {
        return Err(Error::OutputLimit);
    }
    Ok(bytes)
}
/// No shell, no replay. Dropping this future also kills the process group.
pub async fn execute(program: &str, args: &[String], timeout_secs: u64) -> Result<ExecResult> {
    if !(1..=MAX_EXEC_SECONDS).contains(&timeout_secs) {
        return Err(Error::InvalidTimeout);
    }
    let start = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let _group = ProcessGroup(child.id().expect("newly spawned child has a pid"));
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let result = timeout(Duration::from_secs(timeout_secs), async {
        tokio::try_join!(read_output(stdout), read_output(stderr), async {
            Ok::<_, Error>(child.wait().await?)
        })
    })
    .await
    .map_err(|_| Error::Timeout)??;
    Ok(ExecResult {
        stdout: result.0,
        stderr: result.1,
        exit_code: result.2.code(),
        signal: result.2.signal(),
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn argv_is_literal_and_exit_status_preserved() {
        let result = execute("printf", &["%s".into(), "$(false); hello world".into()], 2)
            .await
            .unwrap();
        assert_eq!(result.stdout, b"$(false); hello world");
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(
            execute("sh", &["-c".into(), "exit 7".into()], 2)
                .await
                .unwrap()
                .exit_code,
            Some(7)
        );
    }
    #[tokio::test]
    async fn output_and_runtime_are_bounded() {
        assert!(
            execute("yes", &[], 2)
                .await
                .unwrap_err()
                .to_string()
                .contains("output exceeded")
        );
        assert!(
            execute("sleep", &["5".into()], 1)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }
}
