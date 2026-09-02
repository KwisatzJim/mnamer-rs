use crate::model::RenamePlan;
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub(crate) struct RunLog {
    writer: Option<BufWriter<File>>,
}

#[derive(Serialize)]
pub(crate) struct LogRecord {
    source: String,
    destination: Option<String>,
    status: &'static str,
    reason: String,
}

impl RunLog {
    // A failed group may have rolled back, failed rollback, or stopped before a
    // subtitle was attempted. Report the group outcome, not an assumed file state.
    pub(crate) fn record_group(
        &mut self,
        plan: &RenamePlan,
        status: &'static str,
        reason: &str,
    ) -> Result<()> {
        self.record(&plan.source, Some(&plan.destination), status, reason)?;
        for subtitle in &plan.subtitles {
            self.record(
                &subtitle.source,
                Some(&subtitle.destination),
                status,
                &format!("subtitle rename group {status}: {reason}"),
            )?;
        }
        Ok(())
    }

    pub(crate) fn new(path: Option<&Path>) -> Result<Self> {
        let writer = match path {
            Some(path) => {
                if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)
                        .context("failed to create log file directory")?;
                }
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .with_context(|| {
                        format!(
                            "failed to create new log file {}; existing files are never overwritten—choose an unused log path",
                            path.display()
                        )
                    })?;
                Some(BufWriter::new(file))
            }
            None => None,
        };
        Ok(Self { writer })
    }

    pub(crate) fn record(
        &mut self,
        source: &Path,
        destination: Option<&Path>,
        status: &'static str,
        reason: &str,
    ) -> Result<()> {
        let Some(writer) = &mut self.writer else {
            return Ok(());
        };
        let record = LogRecord {
            source: source.display().to_string(),
            destination: destination.map(|path| path.display().to_string()),
            status,
            reason: reason.to_string(),
        };
        serde_json::to_writer(&mut *writer, &record).context("failed to write log record")?;
        writer
            .write_all(b"\n")
            .context("failed to write log record")?;
        writer.flush().context("failed to flush log record")?;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        if let Some(writer) = &mut self.writer {
            writer
                .flush()
                .context("failed to finish writing log file")?;
        }
        Ok(())
    }
}
