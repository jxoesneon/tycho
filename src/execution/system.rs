//! System subprocess execution with timeout constraints.

use crate::error::{Error, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
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
    pub async fn run_command(
        cmd: &str,
        args: &[&str],
        duration: Duration,
    ) -> Result<SystemCommandResult> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::Io)?;

        let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf).await;
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf).await;
            buf
        });

        match timeout(duration, child.wait()).await {
            Ok(Ok(status)) => {
                let stdout = stdout_task.await.unwrap_or_default();
                let stderr = stderr_task.await.unwrap_or_default();
                Ok(SystemCommandResult {
                    stdout: String::from_utf8_lossy(&stdout).trim().to_string(),
                    stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
                    exit_code: status.code(),
                    timed_out: false,
                })
            }
            Ok(Err(e)) => {
                stdout_task.abort();
                stderr_task.abort();
                Err(Error::Io(e))
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                Ok(SystemCommandResult {
                    stdout: String::new(),
                    stderr: "Process timed out".to_string(),
                    exit_code: None,
                    timed_out: true,
                })
            }
        }
    }

    async fn pipewire_volume(args: &[&str]) -> Result<()> {
        let res = Self::run_command("wpctl", args, Duration::from_secs(5)).await;
        match res {
            Ok(r) if !r.timed_out && r.exit_code == Some(0) => Ok(()),
            _ => {
                // Fallback for systems without WirePlumber (PulseAudio only).
                let mut pactl_args = vec!["set-sink-volume", "@DEFAULT_SINK@"];
                pactl_args.extend_from_slice(&args[2..]);
                if args[0] == "set-mute" {
                    pactl_args = vec!["set-sink-mute", "@DEFAULT_SINK@", args[2]];
                }
                let res = Self::run_command("pactl", &pactl_args, Duration::from_secs(5)).await?;
                if res.timed_out || res.exit_code != Some(0) {
                    return Err(Error::Audio(format!(
                        "volume control failed (wpctl/pactl): {}",
                        res.stderr
                    )));
                }
                Ok(())
            }
        }
    }

    /// Raises the default output volume by `percent` points via wpctl/pactl.
    pub async fn volume_up(percent: u32) -> Result<()> {
        Self::pipewire_volume(&[
            "set-volume",
            "@DEFAULT_AUDIO_SINK@",
            &format!("{}%+", percent),
        ])
        .await
    }

    /// Lowers the default output volume by `percent` points via wpctl/pactl.
    pub async fn volume_down(percent: u32) -> Result<()> {
        Self::pipewire_volume(&[
            "set-volume",
            "@DEFAULT_AUDIO_SINK@",
            &format!("{}%-", percent),
        ])
        .await
    }

    /// Toggles mute on the default output sink.
    pub async fn toggle_mute() -> Result<()> {
        Self::pipewire_volume(&["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).await
    }

    /// Sets mute explicitly — "unmute" must not toggle a live sink off.
    /// Returns the resulting audible state (`true` = now muted).
    pub async fn set_mute(muted: bool) -> Result<bool> {
        if Self::is_muted().await.unwrap_or(false) == muted {
            return Ok(muted);
        }
        Self::pipewire_volume(&[
            "set-mute",
            "@DEFAULT_AUDIO_SINK@",
            if muted { "1" } else { "0" },
        ])
        .await?;
        Ok(muted)
    }

    /// Sets the default output volume to an absolute percentage.
    pub async fn volume_set(percent: u32) -> Result<()> {
        Self::pipewire_volume(&[
            "set-volume",
            "@DEFAULT_AUDIO_SINK@",
            &format!("{}%", percent.min(150)),
        ])
        .await
    }

    /// Current mute state of the default sink (wpctl or pactl).
    pub async fn is_muted() -> Result<bool> {
        let res = Self::run_command(
            "wpctl",
            &["get-volume", "@DEFAULT_AUDIO_SINK@"],
            Duration::from_secs(5),
        )
        .await?;
        if !res.timed_out && res.exit_code == Some(0) {
            return Ok(res.stdout.contains("[MUTED]"));
        }
        let res = Self::run_command(
            "pactl",
            &["get-sink-mute", "@DEFAULT_SINK@"],
            Duration::from_secs(5),
        )
        .await?;
        Ok(res.stdout.to_lowercase().contains("yes"))
    }

    /// Launches a detached application process. A reaper task awaits the
    /// child so exited apps don't linger as zombies under the daemon.
    pub async fn launch(program: &str) -> Result<()> {
        let mut child = Command::new(program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Audio(format!("failed to launch '{}': {}", program, e)))?;
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Ok(())
    }
}
