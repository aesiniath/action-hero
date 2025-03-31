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

fn form_record_directory(prefix: &str, config: &API) -> PathBuf {
    let directory = format!(
        "{}/{}/{}/{}",
        prefix, config.owner, config.repository, config.workflow
    );

    let path = Path::new(&directory);
    path.to_path_buf()
}

fn form_record_filename(directory: &Path, run: &WorkflowRun) -> PathBuf {
    let id = format!("{}", run.run_id);

    let path = directory.join(id);
    path
}

pub(crate) fn check_is_submitted(config: &API, run: &WorkflowRun) -> Result<bool> {
    let directory = form_record_directory(PREFIX, config);
    if !directory.exists() {
        std::fs::create_dir(&directory)?;
    }

    let filename = form_record_filename(&directory, run);

    Ok(false)
}
