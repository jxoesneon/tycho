//! System subprocess execution with timeout constraints.

use crate::error::{Error, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

pub struct SystemExecutor;

impl SystemExecutor {
    pub async fn run_command(cmd: &str, args: &[&str], duration: Duration) -> Result<SystemCommandResult> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Io(e))?;

        match timeout(duration, child.wait_with_output()).await {
            Ok(Ok(output)) => Ok(SystemCommandResult {
                stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                exit_code: output.status.code(),
                timed_out: false,
            }),
            Ok(Err(e)) => Err(Error::Io(e)),
            Err(_) => {
                let _ = child.kill().await;
                Ok(SystemCommandResult {
                    stdout: String::new(),
                    stderr: "Process timed out".to_string(),
                    exit_code: None,
                    timed_out: true,
                })
            }
        }
    }
}
