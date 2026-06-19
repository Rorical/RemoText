use std::{
    path::{Path, PathBuf},
    process,
};

use anyhow::{Context, Result};

pub async fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create parent directory {}", parent.display()))?;
    }
    Ok(())
}

pub fn temp_sibling(path: &Path, label: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("remotext-file");
    let random: u64 = rand::random();
    parent.join(format!(
        ".remotext-{label}-{}-{random}-{name}.tmp",
        process::id()
    ))
}
