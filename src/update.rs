use std::{
    env, fs,
    io::{self},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const REPO: &str = "Rorical/RemoText";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub fn check_for_update(current: &str) -> Result<Option<String>> {
    let release = fetch_latest_release()?;
    let latest = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);

    if latest != current {
        Ok(Some(latest.to_string()))
    } else {
        Ok(None)
    }
}

pub fn self_update() -> Result<String> {
    let current_exe =
        env::current_exe().context("determine current executable path")?;

    let release = fetch_latest_release()?;

    let latest = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);

    if latest == CURRENT_VERSION {
        bail!("already on latest version v{latest}");
    }

    let asset_name = asset_name_for_current_platform()?;
    let filename = if cfg!(windows) {
        format!("{asset_name}.zip")
    } else {
        format!("{asset_name}.tar.gz")
    };

    let download_url = release
        .assets
        .iter()
        .find(|a| a.name == filename)
        .map(|a| a.browser_download_url.clone())
        .with_context(|| {
            format!("release asset {filename} not found for this platform")
        })?;

    let tmp = temp_dir()?;
    let archive_path = tmp.join(&filename);

    download_file(&download_url, &archive_path)?;

    let binary_name = if cfg!(windows) {
        "remotext.exe"
    } else {
        "remotext"
    };
    extract_binary(&archive_path, binary_name, &tmp)?;
    let new_binary = tmp.join(binary_name);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755))
            .context("set executable permission on new binary")?;
    }

    replace_binary(&new_binary, &current_exe)?;

    let _ = fs::remove_dir_all(&tmp);
    Ok(latest.to_string())
}

fn fetch_latest_release() -> Result<GithubRelease> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = ureq::get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "RemoText-updater")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .context("fetch latest release from GitHub")?;

    let release: GithubRelease = response
        .body_mut()
        .read_json()
        .context("parse GitHub release JSON")?;
    Ok(release)
}

fn asset_name_for_current_platform() -> Result<String> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        bail!("unsupported operating system for self-update");
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        bail!("unsupported architecture for self-update");
    };

    Ok(format!("remotext-{os}-{arch}"))
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    let response = ureq::get(url)
        .header("User-Agent", "RemoText-updater")
        .call()
        .with_context(|| format!("download {url}"))?;

    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(dest)
        .with_context(|| format!("create download file {}", dest.display()))?;
    io::copy(&mut reader, &mut file).context("write downloaded file")?;
    Ok(())
}

fn extract_binary(archive: &Path, binary_name: &str, out_dir: &Path) -> Result<()> {
    if cfg!(windows) {
        extract_zip(archive, out_dir)
    } else {
        extract_tar_gz(archive, binary_name, out_dir)
    }
}

fn extract_tar_gz(archive: &Path, binary_name: &str, out_dir: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(out_dir)
        .arg(binary_name)
        .status()
        .context("run tar to extract binary")?;

    if !status.success() {
        bail!("tar extraction failed");
    }
    Ok(())
}

#[cfg(windows)]
fn extract_zip(archive: &Path, out_dir: &Path) -> Result<()> {
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive.display(),
                out_dir.display()
            ),
        ])
        .status()
        .context("run powershell to extract zip")?;

    if !status.success() {
        bail!("zip extraction failed");
    }
    Ok(())
}

#[cfg(not(windows))]
fn extract_zip(_archive: &Path, _out_dir: &Path) -> Result<()> {
    bail!("zip extraction not supported on this platform")
}

#[cfg(unix)]
fn replace_binary(new: &Path, current: &Path) -> Result<()> {
    let backup = current.with_extension("old");
    if backup.exists() {
        fs::remove_file(&backup).ok();
    }

    if current.exists() {
        fs::rename(current, &backup)
            .context("backup current binary")?;
    }

    fs::rename(new, current)
        .context("install new binary")?;

    let _ = fs::remove_file(&backup);
    Ok(())
}

#[cfg(windows)]
fn replace_binary(new: &Path, current: &Path) -> Result<()> {
    let dest = current.with_file_name("remotext.exe.new");
    fs::rename(new, &dest).ok();
    eprintln!("New binary saved to: {}", dest.display());
    eprintln!("Please manually replace {} and restart.", current.display());
    Ok(())
}

fn temp_dir() -> Result<PathBuf> {
    let dir = env::temp_dir().join(format!("remotext-update-{}", std::process::id()));
    fs::create_dir_all(&dir).context("create temp directory for update")?;
    Ok(dir)
}
