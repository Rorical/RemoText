use std::{
    path::{Path, PathBuf},
    process,
};

use anyhow::{Context, Result, bail};

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

#[allow(dead_code)]
pub fn canonicalize_or_bail(path: &Path, base: &Path) -> Result<PathBuf> {
    let base = base
        .canonicalize()
        .with_context(|| format!("canonicalize base directory {}", base.display()))?;
    let resolved = base.join(path);
    let resolved = resolved
        .canonicalize()
        .with_context(|| format!("canonicalize path {}", resolved.display()))?;
    if !resolved.starts_with(&base) {
        bail!(
            "path escapes allowed directory: {}",
            path.display()
        );
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn canonicalize_or_bail_works_within_base() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::write(base.join("a.txt"), b"hello").unwrap();
        let result = canonicalize_or_bail(Path::new("a.txt"), base);
        assert!(result.is_ok());
    }

    #[test]
    fn canonicalize_or_bail_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let result = canonicalize_or_bail(Path::new("../etc/passwd"), base);
        assert!(result.is_err());
    }
}
