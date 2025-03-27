use anyhow::Result;
use std::{self, path::Path};

pub(crate) fn ensure_record_directory() -> Result<()> {
    let path = Path::new("record");
    if !path.exists() {
        std::fs::create_dir(path)?;
    }
    Ok(())
}
