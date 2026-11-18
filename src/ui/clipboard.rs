use anyhow::{Context, Result};

pub(super) fn copy_to_clipboard(code: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let mut child = Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .context("starting macOS clipboard")?;
        let written = child
            .stdin
            .take()
            .context("clipboard stdin")?
            .write_all(code.as_bytes());
        let status = child.wait()?;
        written?;
        anyhow::ensure!(status.success(), "clipboard command failed");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = code;
        anyhow::bail!("clipboard currently supports macOS only")
    }
}
