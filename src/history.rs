use anyhow::Result;
use std::{
    self,
    path::{Path, PathBuf},
};
use tracing::{debug, info};

use crate::github::{API, WorkflowRun};

pub(crate) fn ensure_record_directory(prefix: &str) -> Result<()> {
    let path = Path::new(prefix);
    if !path.exists() {
        std::fs::create_dir(path)?;
    }
    Ok(())
}

pub(crate) fn form_record_filename(prefix: &str, config: &API, run: &WorkflowRun) -> PathBuf {
    let id = format!("{}", run.run_id);

    let name = format!(
        "{}/{}/{}/{}",
        prefix, config.owner, config.repository, config.workflow
    );

    let directory = Path::new(&name);
    let path = directory.join(id);
    path
}

pub(crate) fn check_is_submitted(path: &Path) -> Result<bool> {
    let directory = path
        .parent()
        .unwrap();

    debug!(?path);

    if !directory.exists() {
        std::fs::create_dir_all(&directory)?;
    }

    let probe = path.exists();
    Ok(probe)
}

pub(crate) fn mark_run_submitted(path: &Path) -> Result<()> {
    if !path.exists() {
        // create empty file
        info!("Recording Run completion");
        std::fs::write(&path, [])?;
    }

    Ok(())
}
