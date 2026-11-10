use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// Create the temporary file beside the destination so rename remains atomic.
pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().context("destination has no file name")?;
    let mut temporary: Option<PathBuf> = None;
    let result = (|| -> Result<()> {
        let mut file = loop {
            let mut candidate_name = name.to_os_string();
            candidate_name.push(format!(
                ".{}.{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let candidate = parent.join(candidate_name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    temporary = Some(candidate);
                    break file;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("creating temporary file"),
            }
        };
        file.write_all(data).context("writing temporary file")?;
        file.sync_all().context("syncing temporary file")?;
        drop(file);
        fs::rename(temporary.as_ref().unwrap(), path)
            .with_context(|| format!("replacing {}", path.display()))?;
        temporary = None;
        Ok(())
    })();
    if let Some(path) = temporary {
        let _ = fs::remove_file(path);
    }
    result
}
