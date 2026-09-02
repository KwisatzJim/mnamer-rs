use crate::model::RenamePlan;
use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;
use tempfile::NamedTempFile;

/// Apply a video and its subtitles as one group, rolling back completed moves
/// if a later subtitle fails.
pub(crate) fn apply_rename_group(plan: &RenamePlan, force_copy: bool) -> Result<()> {
    let mut completed: Vec<(&Path, &Path)> = Vec::new();

    apply_move(&plan.source, &plan.destination, force_copy)?;
    completed.push((&plan.source, &plan.destination));

    for subtitle in &plan.subtitles {
        if let Err(error) = apply_move(&subtitle.source, &subtitle.destination, force_copy) {
            return match rollback_moves(&completed) {
                Ok(()) => Err(error).with_context(|| {
                    format!(
                        "subtitle move failed for {}; completed group changes were rolled back",
                        subtitle.source.display()
                    )
                }),
                Err(rollback_error) => Err(error).with_context(|| {
                    format!(
                        "subtitle move failed for {}; rollback also failed: {rollback_error:#}",
                        subtitle.source.display()
                    )
                }),
            };
        }
        completed.push((&subtitle.source, &subtitle.destination));
    }

    Ok(())
}

pub(crate) fn rollback_moves(completed: &[(&Path, &Path)]) -> Result<()> {
    let mut errors = Vec::new();
    for (source, destination) in completed.iter().rev() {
        if let Err(error) = apply_move(destination, source, false) {
            errors.push(format!(
                "{} -> {}: {error:#}",
                destination.display(),
                source.display()
            ));
        }
    }
    if !errors.is_empty() {
        anyhow::bail!(errors.join("; "));
    }
    Ok(())
}

/// Move `from` to `to` without ever replacing an existing destination.
/// A hard link provides an atomic same-filesystem move; filesystems that do
/// not support it fall back to the no-clobber temporary-copy path.
pub(crate) fn apply_move(from: &Path, to: &Path, force_copy: bool) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).context("failed to create destination directory")?;
    }
    if std::fs::symlink_metadata(to).is_ok() {
        anyhow::bail!("destination already exists: {}", to.display());
    }
    if !force_copy {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                if let Err(error) = std::fs::remove_file(from) {
                    return match std::fs::remove_file(to) {
                        Ok(()) => Err(error).with_context(|| {
                            format!(
                                "could not remove original {}; newly created destination was rolled back",
                                from.display()
                            )
                        }),
                        Err(cleanup_error) => Err(error).with_context(|| {
                            format!(
                                "could not remove original {}; cleanup of newly created destination {} also failed: {cleanup_error}",
                                from.display(),
                                to.display()
                            )
                        }),
                    };
                }
                return Ok(());
            }
            Err(error) if std::fs::symlink_metadata(to).is_ok() => {
                anyhow::bail!(
                    "destination appeared before the move completed and was left unchanged: {} ({error})",
                    to.display()
                );
            }
            Err(_) => {}
        }
    }
    copy_then_delete(from, to)?;
    Ok(())
}

pub(crate) fn copy_then_delete(from: &Path, to: &Path) -> Result<()> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    let mut source = File::open(from).context("failed to open source file for copying")?;
    let source_permissions = source
        .metadata()
        .context("failed to read source file metadata")?
        .permissions();
    let mut temporary =
        NamedTempFile::new_in(parent).context("failed to create temporary destination file")?;

    std::io::copy(&mut source, temporary.as_file_mut())
        .context("failed while copying file to temporary destination")?;
    temporary
        .as_file_mut()
        .set_permissions(source_permissions)
        .context("failed to preserve source file permissions")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("failed to finish writing temporary destination file")?;

    temporary
        .persist_noclobber(to)
        .map_err(|error| error.error)
        .context("failed to publish copied file at destination")?;
    if let Err(error) = std::fs::remove_file(from) {
        return match std::fs::remove_file(to) {
            Ok(()) => Err(error).context(
                "failed to remove original file after copy; published destination was rolled back",
            ),
            Err(cleanup_error) => Err(error).context(format!(
                "failed to remove original file after copy; cleanup of published destination also failed: {cleanup_error}"
            )),
        };
    }
    Ok(())
}
