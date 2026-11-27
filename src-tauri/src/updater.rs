//! In-app GitHub Releases updater.
//!
//! Checks `https://api.github.com/repos/ipepio/nabla/releases/latest` directly from
//! Rust, matches the installer for the current OS/arch, streams the download with
//! progress events, verifies the SHA-256 digest published by GitHub, and installs
//! the new version without going through a browser (so macOS does not attach
//! `com.apple.quarantine`).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_API_URL: &str = "https://api.github.com/repos/ipepio/nabla/releases/latest";
const CHECK_TIMEOUT: Duration = Duration::from_secs(12);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
pub const PROGRESS_EVENT: &str = "updater-progress";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub available: bool,
    pub release_url: String,
    pub notes: String,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
    pub asset_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressEvent {
    /// `"downloading"` | `"verifying"` | `"installing"` | `"relaunching"` | `"error"`
    pub stage: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let clean = raw.trim().trim_start_matches('v').trim_start_matches('V');
    let core = clean.split('-').next()?.split('+').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn pick_asset_for_target(
    assets: &[Value],
    os: &str,
    arch: &str,
) -> Option<(String, String, Option<u64>, Option<String>)> {
    let preferred_suffixes: &[&str] = match (os, arch) {
        ("macos", "aarch64") => &["_aarch64.dmg", ".dmg"],
        ("macos", "x86_64") => &["_x64.dmg", ".dmg"],
        ("windows", _) => &["-setup.exe", ".exe"],
        ("linux", _) => &["_amd64.deb", ".deb"],
        _ => &[],
    };

    for suffix in preferred_suffixes {
        for asset in assets {
            let Some(name) = asset.get("name").and_then(Value::as_str) else {
                continue;
            };
            if !name.to_ascii_lowercase().ends_with(suffix) {
                continue;
            }
            let Some(url) = asset
                .get("browser_download_url")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let size = asset.get("size").and_then(Value::as_u64);
            let sha256 = asset
                .get("digest")
                .and_then(Value::as_str)
                .and_then(|d| d.strip_prefix("sha256:"))
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| h.len() == 64);
            return Some((name.to_string(), url, size, sha256));
        }
    }
    None
}

pub fn parse_release_payload(body: &str, current_version: &str) -> Result<UpdateInfo, String> {
    let val: Value =
        serde_json::from_str(body).map_err(|e| format!("invalid GitHub release JSON: {e}"))?;
    let tag = val
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "release missing tag_name".to_string())?;
    let latest_version = tag.trim().trim_start_matches('v').to_string();
    let available = is_newer_version(&latest_version, current_version);
    let release_url = val
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or("https://github.com/ipepio/nabla/releases/latest")
        .to_string();
    let notes = val
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    let assets = val
        .get("assets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let picked = pick_asset_for_target(assets, std::env::consts::OS, std::env::consts::ARCH);
    let (asset_name, asset_url, asset_size, asset_sha256) = match picked {
        Some((n, u, s, h)) => (Some(n), Some(u), s, h),
        None => (None, None, None, None),
    };

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version,
        available,
        release_url,
        notes,
        asset_name,
        asset_url,
        asset_size,
        asset_sha256,
    })
}

pub async fn check_for_update() -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let resp = client
        .get(RELEASES_API_URL)
        .header("User-Agent", format!("Nabla/{CURRENT_VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Could not reach GitHub Releases: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub Releases returned HTTP {}", resp.status().as_u16()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Failed reading release response: {e}"))?;
    parse_release_payload(&body, CURRENT_VERSION)
}

fn emit_progress(
    app: &tauri::AppHandle,
    stage: &str,
    downloaded: u64,
    total: Option<u64>,
    message: Option<String>,
) {
    let _ = app.emit(
        PROGRESS_EVENT,
        UpdateProgressEvent {
            stage: stage.to_string(),
            downloaded,
            total,
            message,
        },
    );
}

// ---------------------------------------------------------------------------
// Minimal pure-Rust SHA-256 (FIPS 180-4) — zero extra dependencies.
// ---------------------------------------------------------------------------

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                0x1f83d9ab, 0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn finalize_hex(mut self) -> String {
        let bit_len = self.total_len * 8;
        self.update(&[0x80]);
        if self.buf_len > 56 {
            let pad = 64 - self.buf_len;
            self.update(&vec![0u8; pad]);
        }
        let zeros = 56 - self.buf_len;
        if zeros > 0 {
            self.buf[self.buf_len..56].fill(0);
        }
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        self.compress(&block);
        self.state.iter().map(|w| format!("{w:08x}")).collect()
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

pub async fn install_update(app: &tauri::AppHandle, info: &UpdateInfo) -> Result<(), String> {
    let url = info
        .asset_url
        .as_deref()
        .ok_or_else(|| "No installer asset available for this platform.".to_string())?;
    let name = info
        .asset_name
        .as_deref()
        .ok_or_else(|| "Missing installer filename.".to_string())?;
    if !url.starts_with("https://github.com/ipepio/nabla/releases/download/") {
        return Err("Refused untrusted update download URL.".to_string());
    }

    let tmp_dir = std::env::temp_dir().join("nabla-update");
    fs::create_dir_all(&tmp_dir).map_err(|e| format!("cannot create temp dir: {e}"))?;
    let dest_file = tmp_dir.join(name);

    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    emit_progress(app, "downloading", 0, info.asset_size, None);
    let resp = client
        .get(url)
        .header("User-Agent", format!("Nabla/{CURRENT_VERSION}"))
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download returned HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length().or(info.asset_size);

    let mut file = File::create(&dest_file).map_err(|e| format!("cannot create file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_emitted: u64 = 0;
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&bytes)
            .map_err(|e| format!("Write failed: {e}"))?;
        hasher.update(&bytes);
        downloaded += bytes.len() as u64;
        if downloaded.saturating_sub(last_emitted) >= 256 * 1024 {
            last_emitted = downloaded;
            emit_progress(app, "downloading", downloaded, total, None);
        }
    }
    file.flush().map_err(|e| format!("Flush failed: {e}"))?;
    drop(file);
    emit_progress(app, "verifying", downloaded, total, None);

    if let Some(expected) = info.asset_sha256.as_deref() {
        let actual = hasher.finalize_hex();
        if actual != expected {
            let _ = fs::remove_file(&dest_file);
            return Err("Downloaded update failed SHA-256 verification.".to_string());
        }
    }

    emit_progress(app, "installing", downloaded, total, None);
    apply_platform_installer(app, &dest_file, &tmp_dir)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn resolve_macos_app_target() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors() {
            if ancestor.extension().and_then(|e| e.to_str()) == Some("app") {
                return ancestor.to_path_buf();
            }
        }
    }
    PathBuf::from("/Applications/Nabla.app")
}

#[cfg(target_os = "macos")]
fn apply_platform_installer(
    app: &tauri::AppHandle,
    dmg_path: &Path,
    tmp_dir: &Path,
) -> Result<(), String> {
    let mount_point = tmp_dir.join("mnt");
    let _ = fs::remove_dir_all(&mount_point);
    fs::create_dir_all(&mount_point).map_err(|e| format!("cannot create mount dir: {e}"))?;

    let attach = std::process::Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-mountpoint",
            mount_point.to_str().unwrap_or("/tmp/nabla-mnt"),
            dmg_path.to_str().unwrap_or(""),
        ])
        .output()
        .map_err(|e| format!("hdiutil attach failed: {e}"))?;
    if !attach.status.success() {
        return Err(format!(
            "Could not mount DMG: {}",
            String::from_utf8_lossy(&attach.stderr).trim()
        ));
    }

    let source_app = mount_point.join("Nabla.app");
    if !source_app.is_dir() {
        let _ = std::process::Command::new("hdiutil")
            .args(["detach", "-force", mount_point.to_str().unwrap_or("")])
            .status();
        return Err("Nabla.app was not found inside the downloaded DMG.".to_string());
    }

    let target_app = resolve_macos_app_target();
    let staging_app = target_app.with_extension("app.new");
    let backup_app = target_app.with_extension("app.old");
    let _ = fs::remove_dir_all(&staging_app);
    let _ = fs::remove_dir_all(&backup_app);

    let copy_status = std::process::Command::new("ditto")
        .args([&source_app, &staging_app])
        .status();
    let _ = std::process::Command::new("hdiutil")
        .args(["detach", "-force", mount_point.to_str().unwrap_or("")])
        .status();

    if !copy_status.map(|s| s.success()).unwrap_or(false) {
        let _ = fs::remove_dir_all(&staging_app);
        return Err(format!(
            "Could not copy update into {} (check folder permissions).",
            target_app.display()
        ));
    }

    if target_app.exists() {
        fs::rename(&target_app, &backup_app)
            .map_err(|e| format!("Could not move current app aside: {e}"))?;
    }
    if let Err(err) = fs::rename(&staging_app, &target_app) {
        let _ = fs::rename(&backup_app, &target_app);
        return Err(format!("Could not install new app bundle: {err}"));
    }
    let _ = fs::remove_dir_all(&backup_app);
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine", target_app.to_str().unwrap_or("")])
        .status();
    let _ = fs::remove_file(dmg_path);

    emit_progress(app, "relaunching", 0, None, None);
    let target_str = target_app.to_string_lossy().to_string();
    let _ = std::process::Command::new("sh")
        .args(["-c", "sleep 0.5 && open -n \"$1\"", "_", &target_str])
        .spawn();
    app.exit(0);
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_platform_installer(
    app: &tauri::AppHandle,
    exe_path: &Path,
    _tmp_dir: &Path,
) -> Result<(), String> {
    emit_progress(app, "relaunching", 0, None, None);
    std::process::Command::new(exe_path)
        .spawn()
        .map_err(|e| format!("Could not launch installer: {e}"))?;
    app.exit(0);
    Ok(())
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn apply_platform_installer(
    _app: &tauri::AppHandle,
    deb_path: &Path,
    _tmp_dir: &Path,
) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(deb_path)
        .spawn()
        .map_err(|e| format!("Could not open .deb package: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison_handles_v_prefix_and_ordering() {
        assert!(is_newer_version("v0.5.0", "0.4.0"));
        assert!(is_newer_version("0.4.1", "0.4.0"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(!is_newer_version("0.4.0", "0.4.0"));
        assert!(!is_newer_version("0.3.9", "0.4.0"));
    }

    #[test]
    fn parses_github_release_and_picks_platform_asset() {
        let payload = r#"{
            "tag_name": "v0.5.0",
            "html_url": "https://github.com/ipepio/nabla/releases/tag/v0.5.0",
            "body": "Bug fixes and improvements",
            "assets": [
                {
                    "name": "Nabla_0.5.0_aarch64.dmg",
                    "browser_download_url": "https://github.com/ipepio/nabla/releases/download/v0.5.0/Nabla_0.5.0_aarch64.dmg",
                    "size": 190000000,
                    "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                {
                    "name": "Nabla_0.5.0_x64-setup.exe",
                    "browser_download_url": "https://github.com/ipepio/nabla/releases/download/v0.5.0/Nabla_0.5.0_x64-setup.exe",
                    "size": 50000000
                },
                {
                    "name": "Nabla_0.5.0_amd64.deb",
                    "browser_download_url": "https://github.com/ipepio/nabla/releases/download/v0.5.0/Nabla_0.5.0_amd64.deb",
                    "size": 95000000
                }
            ]
        }"#;
        let info = parse_release_payload(payload, "0.4.0").unwrap();
        assert!(info.available);
        assert_eq!(info.latest_version, "0.5.0");
        assert!(info.asset_name.is_some());
        assert!(info.asset_url.is_some());
    }

    #[test]
    fn sha256_matches_standard_test_vector() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            h.finalize_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
