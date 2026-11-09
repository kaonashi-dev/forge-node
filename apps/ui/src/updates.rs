//! The in-app updater: check the release manifest, classify a release by the
//! protocol it speaks, download and verify the minisign-signed bundle, and
//! swap it in. The daemon is never replaced: a soft update relaunches the GUI
//! into the daemon still running, a hard one is refused in place.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;

use crate::app::AppState;

pub const ENDPOINT: &str =
    "https://github.com/kaonashi-dev/forge-node/releases/latest/download/latest-rune.json";
pub const UPDATER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDNCRjAzMEJBQTgwQTRCNDcKUldSSFN3cW91akR3TzhkUVZ5WU5zU1hKNjkxL3lEZTV0NjVKdVNEYnhWTzhFcHVhR3BoT2JEY0wK";
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const LAUNCH_DELAY: Duration = Duration::from_secs(30);
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 250 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 500 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub const APP_VERSION: &str = env!("FORGE_NODE_VERSION");

static APP: OnceLock<rune::AppHandle<AppState>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateKind {
    Soft,
    Hard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub kind: UpdateKind,
    pub platform: &'static str,
    pub url: String,
    pub signature: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes: String,
        kind: UpdateKind,
    },
    Downloading {
        percent: u8,
    },
    Installing,
    Failed {
        message: String,
    },
}

#[derive(Default)]
pub struct Updates {
    pub state: UpdateState,
    pub pending: Option<Release>,
    pub last_checked: Option<Instant>,
    /// A check or install is on its worker; one at a time.
    pub working: bool,
    /// The gesture was the menu or the Settings button, so "up to date"
    /// is worth reporting; a background check stays quiet.
    pub asked: bool,
    pub fixture: bool,
}

impl AppState {
    pub fn check_for_updates(&mut self, force: bool) {
        if self.updates.working {
            return;
        }
        if !force
            && self
                .updates
                .last_checked
                .is_some_and(|at| at.elapsed() < CHECK_INTERVAL)
        {
            return;
        }
        self.updates.asked = force || self.updates.asked;
        if self.updates.fixture {
            self.updates.last_checked = Some(Instant::now());
            self.updates.state = UpdateState::UpToDate;
            return;
        }
        let Some(handle) = APP.get().cloned() else {
            return;
        };
        self.updates.working = true;
        self.updates.state = UpdateState::Checking;
        let spawned = std::thread::Builder::new()
            .name("forge-ui-update-check".into())
            .spawn(move || {
                let found = fetch_release();
                handle.update(move |s, _| {
                    s.updates.working = false;
                    s.updates.last_checked = Some(Instant::now());
                    match found {
                        Ok(Some(release)) => {
                            s.updates.state = UpdateState::Available {
                                version: release.version.clone(),
                                notes: release.notes.clone(),
                                kind: release.kind,
                            };
                            s.updates.pending = Some(release);
                        }
                        Ok(None) => {
                            s.updates.pending = None;
                            s.updates.state = UpdateState::UpToDate;
                        }
                        Err(message) => {
                            s.updates.state = UpdateState::Failed { message };
                        }
                    }
                });
            });
        if spawned.is_err() {
            self.updates.working = false;
            self.updates.state = UpdateState::Failed {
                message: "Could not start the update check.".into(),
            };
        }
    }

    pub fn install_update(&mut self) {
        if self.updates.working {
            return;
        }
        let Some(release) = self.updates.pending.clone() else {
            self.updates.state = UpdateState::Failed {
                message: "No update is pending. Check again.".into(),
            };
            return;
        };
        if let Some(message) = hard_refusal(&release) {
            self.updates.state = UpdateState::Failed { message };
            return;
        }
        let Some(handle) = APP.get().cloned() else {
            return;
        };
        self.updates.working = true;
        self.updates.state = UpdateState::Downloading { percent: 0 };
        let spawned = std::thread::Builder::new()
            .name("forge-ui-update-install".into())
            .spawn(move || {
                let progress_handle = handle.clone();
                let last = std::cell::Cell::new(u8::MAX);
                let archive = download_archive(&release, &|percent| {
                    if percent != last.get() {
                        last.set(percent);
                        progress_handle.update(move |s, _| {
                            s.updates.state = UpdateState::Downloading { percent };
                        });
                    }
                });
                let archive = match archive {
                    Ok(path) => path,
                    Err(message) => {
                        handle.update(move |s, _| {
                            s.updates.working = false;
                            s.updates.state = UpdateState::Failed { message };
                        });
                        return;
                    }
                };
                if let Err(message) = verify_archive(&archive, &release.signature) {
                    let _ = fs::remove_file(&archive);
                    handle.update(move |s, _| {
                        s.updates.working = false;
                        s.updates.state = UpdateState::Failed { message };
                    });
                    return;
                }
                handle.update(|s, _| {
                    s.updates.state = UpdateState::Installing;
                });
                let installed = bundle_path().and_then(|live| {
                    let parent = live
                        .parent()
                        .ok_or_else(|| "The app bundle has no parent folder.".to_string())?;
                    let (staged, staging) = extract_archive(&archive, parent)?;
                    let result = install_bundle(&staged, &live).map(|()| live);
                    let _ = fs::remove_dir_all(staging);
                    result
                });
                let _ = fs::remove_file(&archive);
                match installed {
                    Ok(live) => {
                        let opened = std::process::Command::new("/usr/bin/open")
                            .arg("-n")
                            .arg(&live)
                            .status()
                            .map_err(|error| {
                                format!("Could not relaunch {}: {error}", live.display())
                            });
                        match opened {
                            Ok(status) if status.success() => handle.quit(),
                            Ok(status) => handle.update(move |s, _| {
                                s.updates.working = false;
                                s.updates.state = UpdateState::Failed {
                                    message: format!("The relaunch exited with {status}."),
                                };
                            }),
                            Err(message) => handle.update(move |s, _| {
                                s.updates.working = false;
                                s.updates.state = UpdateState::Failed { message };
                            }),
                        }
                    }
                    Err(message) => handle.update(move |s, _| {
                        s.updates.working = false;
                        s.updates.state = UpdateState::Failed { message };
                    }),
                }
            });
        if spawned.is_err() {
            self.updates.working = false;
            self.updates.state = UpdateState::Failed {
                message: "Could not start the update.".into(),
            };
        }
    }

    /// What "Check for Updates…" does: the row is the answer, so Settings
    /// opens on General and the check reports either way.
    pub fn open_update_check(&mut self) {
        self.open_settings(Some(crate::settings::Section::General));
        self.updates.asked = true;
        self.check_for_updates(true);
    }
}

pub fn install_handle(handle: rune::AppHandle<AppState>) {
    let _ = APP.set(handle);
}

pub fn start_schedule(handle: rune::AppHandle<AppState>, fixture: bool) {
    if fixture {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("forge-ui-update-schedule".into())
        .spawn(move || {
            std::thread::sleep(LAUNCH_DELAY);
            loop {
                let handle = handle.clone();
                handle.update(|s, _| s.check_for_updates(false));
                std::thread::sleep(CHECK_INTERVAL);
            }
        });
    if spawned.is_err() {
        eprintln!("forge-ui: could not start the update schedule");
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    platforms: std::collections::HashMap<String, PlatformEntry>,
}

#[derive(Debug, Deserialize)]
struct PlatformEntry {
    signature: String,
    url: String,
}

/// The keys in the order the release may carry them: the host's own
/// architecture first, then a universal build.
#[must_use]
pub fn platform_keys() -> &'static [&'static str] {
    if cfg!(target_arch = "aarch64") {
        &["darwin-aarch64", "darwin-universal"]
    } else {
        &["darwin-x86_64", "darwin-universal"]
    }
}

/// A release note's first line is the machine-readable protocol; a missing
/// or different line is hard, because guessing "soft" wrongly relaunches the
/// GUI into a handshake the running daemon rejects.
#[must_use]
pub fn classify_notes(body: &str) -> (UpdateKind, String) {
    let mut lines = body.lines();
    let Some(first) = lines.next() else {
        return (UpdateKind::Hard, String::new());
    };
    let Some(value) = first.trim().strip_prefix("protocol:") else {
        return (UpdateKind::Hard, body.trim().to_string());
    };
    let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    match value.trim().parse::<u32>() {
        Ok(version) if version == client::PROTOCOL_VERSION => (UpdateKind::Soft, rest),
        _ => (UpdateKind::Hard, rest),
    }
}

pub fn is_newer(remote: &str, current: &str) -> Result<bool, String> {
    let remote =
        Version::parse(remote).map_err(|error| format!("Invalid release version: {error}"))?;
    let current =
        Version::parse(current).map_err(|error| format!("Invalid app version: {error}"))?;
    Ok(remote > current)
}

fn release_from(manifest: &Manifest, platform: &'static str) -> Result<Option<Release>, String> {
    if !is_newer(&manifest.version, APP_VERSION)? {
        return Ok(None);
    }
    let entry = manifest
        .platforms
        .get(platform)
        .ok_or_else(|| format!("The release has no {platform} build."))?;
    if entry.signature.trim().is_empty() {
        return Err("The release's signature is empty.".into());
    }
    if !entry.url.starts_with("https://") {
        return Err("The release URL is not HTTPS.".into());
    }
    let (kind, notes) = classify_notes(&manifest.notes);
    Ok(Some(Release {
        version: manifest.version.clone(),
        notes,
        kind,
        platform,
        url: entry.url.clone(),
        signature: entry.signature.clone(),
    }))
}

/// Parse a `latest.json` and pick this host's platform, when it is newer.
#[must_use = "a refused manifest is not an update"]
pub fn parse_release(body: &str) -> Result<Option<Release>, String> {
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("The release manifest is over 1 MiB.".into());
    }
    let manifest: Manifest = serde_json::from_str(body)
        .map_err(|error| format!("Could not read the release: {error}"))?;
    if !is_newer(&manifest.version, APP_VERSION)? {
        return Ok(None);
    }
    for platform in platform_keys() {
        if manifest.platforms.contains_key(*platform) {
            return release_from(&manifest, platform);
        }
    }
    Err("The release has no build for this Mac.".into())
}

#[must_use]
pub fn hard_refusal(release: &Release) -> Option<String> {
    (release.kind == UpdateKind::Hard).then(|| {
        format!(
            "{} changes the daemon protocol (this build speaks {}). An in-app update cannot \
             replace the daemon without stopping every session; download and run the full \
             installer instead.",
            release.version,
            client::PROTOCOL_VERSION
        )
    })
}

fn fetch_release() -> Result<Option<Release>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(30))
        .redirects(5)
        .build();
    let response = agent
        .get(ENDPOINT)
        .set("User-Agent", &format!("forge-node/{APP_VERSION}"))
        .call()
        .map_err(|error| format!("Could not check for updates: {error}"))?;
    if !response.get_url().starts_with("https://") {
        return Err("The update endpoint redirected away from HTTPS.".into());
    }
    if let Some(length) = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        && length > MAX_MANIFEST_BYTES
    {
        return Err("The release manifest is over 1 MiB.".into());
    }
    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut body)
        .map_err(|error| format!("Could not read the release: {error}"))?;
    parse_release(&body)
}

fn download_archive(release: &Release, progress: &dyn Fn(u8)) -> Result<PathBuf, String> {
    if !release.url.starts_with("https://") {
        return Err("The download URL is not HTTPS.".into());
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(120))
        .redirects(5)
        .build();
    let response = agent
        .get(&release.url)
        .set("User-Agent", &format!("forge-node/{APP_VERSION}"))
        .call()
        .map_err(|error| format!("Could not download {}: {error}", release.version))?;
    if !response.get_url().starts_with("https://") {
        return Err("The download redirected away from HTTPS.".into());
    }
    let total = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok());
    if total.is_some_and(|length| length > MAX_ARCHIVE_BYTES) {
        return Err("The download is over 250 MiB.".into());
    }
    let path = std::env::temp_dir().join(format!(
        "forge-node-{}-{}.app.tar.gz",
        std::process::id(),
        release.version
    ));
    let mut file =
        File::create(&path).map_err(|error| format!("Could not save the download: {error}"))?;
    let mut reader = response.into_reader().take(MAX_ARCHIVE_BYTES + 1);
    let mut buffer = [0u8; 64 * 1024];
    let mut seen = 0u64;
    let mut last = u8::MAX;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Could not read the download: {error}"))?;
        if read == 0 {
            break;
        }
        seen = seen.saturating_add(read as u64);
        if seen > MAX_ARCHIVE_BYTES {
            let _ = fs::remove_file(&path);
            return Err("The download is over 250 MiB.".into());
        }
        file.write_all(&buffer[..read])
            .map_err(|error| format!("Could not save the download: {error}"))?;
        let percent = total.map_or(0, |length| {
            seen.min(length)
                .checked_mul(100)
                .and_then(|scaled| scaled.checked_div(length))
                .unwrap_or(0) as u8
        });
        if percent != last {
            last = percent;
            progress(percent);
        }
    }
    file.sync_all()
        .map_err(|error| format!("Could not finish the download: {error}"))?;
    progress(100);
    Ok(path)
}

fn updater_key() -> Result<PublicKey, String> {
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(UPDATER_PUBKEY)
        .map_err(|_| "The updater's public key is unreadable.".to_string())?;
    let text = String::from_utf8(decoded)
        .map_err(|_| "The updater's public key is unreadable.".to_string())?;
    PublicKey::decode(&text).map_err(|_| "The updater's public key is unreadable.".to_string())
}

fn verify_archive(path: &Path, signature_text: &str) -> Result<(), String> {
    let key = updater_key()?;
    let signature = Signature::decode(signature_text)
        .map_err(|_| "The release's signature is unreadable; it was not installed.".to_string())?;
    let mut verifier = key.verify_stream(&signature).map_err(|_| {
        "The release's signature is not a prehashed minisign signature.".to_string()
    })?;
    let mut file =
        File::open(path).map_err(|error| format!("Could not reopen the download: {error}"))?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not verify the download: {error}"))?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier
        .finalize()
        .map_err(|_| "The release failed signature verification; it was not installed.".to_string())
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

fn extract_archive(archive_path: &Path, parent: &Path) -> Result<(PathBuf, PathBuf), String> {
    let dir = parent.join(format!(".forge-node-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|error| format!("Could not stage the update: {error}"))?;
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open the download: {error}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|error| format!("The download is not a tar archive: {error}"))?;
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("The download is damaged: {error}"))?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err("The download has too many files.".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| "The download is too large.".to_string())?;
        if total > MAX_UNPACKED_BYTES {
            return Err("The download unpacks to over 500 MiB.".into());
        }
        let path = entry
            .path()
            .map_err(|error| format!("The download has an unreadable path: {error}"))?
            .to_path_buf();
        if !safe_relative(&path) {
            return Err("The download contains an unsafe path.".into());
        }
        let kind = entry.header().entry_type();
        if kind.is_hard_link()
            || kind.is_fifo()
            || kind.is_character_special()
            || kind.is_block_special()
        {
            return Err("The download contains an unsupported file type.".into());
        }
        if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|error| format!("The download has an unreadable link: {error}"))?
                .ok_or_else(|| "The download has a link with no target.".to_string())?;
            if !safe_relative(&target) {
                return Err("The download contains a link that leaves the app.".into());
            }
        }
        entry
            .unpack_in(&dir)
            .map_err(|error| format!("Could not unpack the update: {error}"))?;
    }
    let mut apps = Vec::new();
    for entry in
        fs::read_dir(&dir).map_err(|error| format!("Could not read the staged update: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Could not read the staged update: {error}"))?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "app") && path.is_dir() {
            apps.push(path);
        }
    }
    match apps.as_slice() {
        [app] => Ok((app.clone(), dir)),
        [] => Err("The download does not contain an app.".into()),
        _ => Err("The download contains more than one app.".into()),
    }
}

fn bundle_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not find the running app: {error}"))?;
    executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            "This build does not run from an app bundle; install the release build instead.".into()
        })
}

fn install_bundle(staged: &Path, live: &Path) -> Result<(), String> {
    let parent = live
        .parent()
        .ok_or_else(|| "The app bundle has no parent folder.".to_string())?;
    let name = live
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Forge Node.app");
    let backup = parent.join(format!(".{name}.previous-{}", std::process::id()));
    let _ = fs::remove_dir_all(&backup);
    fs::rename(live, &backup)
        .map_err(|error| format!("Could not move the current app aside: {error}"))?;
    if let Err(error) = fs::rename(staged, live) {
        let _ = fs::rename(&backup, live);
        return Err(format!("Could not install the new app: {error}"));
    }
    let _ = fs::remove_dir_all(&backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "version": "9.8.7",
        "notes": "protocol: 37\nA softer landing.",
        "pub_date": "2026-11-10T00:00:00Z",
        "platforms": {
            "darwin-aarch64": {
                "signature": "untrusted comment: signature from minisign secret key\nRWR…",
                "url": "https://github.com/kaonashi-dev/forge-node/releases/download/v9.8.7/ForgeNode-9.8.7-darwin-aarch64.app.tar.gz"
            },
            "darwin-x86_64": {
                "signature": "intel signature",
                "url": "https://github.com/kaonashi-dev/forge-node/releases/download/v9.8.7/ForgeNode-9.8.7-darwin-x86_64.app.tar.gz"
            }
        }
    }"#;

    #[test]
    fn a_matching_protocol_is_soft_and_the_header_is_not_notes() {
        let (kind, notes) = classify_notes(&format!(
            "protocol: {}\nDrag-reorderable tabs.",
            client::PROTOCOL_VERSION
        ));
        assert_eq!(kind, UpdateKind::Soft);
        assert_eq!(notes, "Drag-reorderable tabs.");
    }

    #[test]
    fn a_missing_or_different_protocol_is_hard() {
        assert_eq!(classify_notes("").0, UpdateKind::Hard);
        let (kind, notes) = classify_notes("Just a human changelog.\nSecond line.");
        assert_eq!(kind, UpdateKind::Hard);
        assert_eq!(notes, "Just a human changelog.\nSecond line.");
        let (kind, _) = classify_notes(&format!(
            "protocol: {}\nnotes",
            client::PROTOCOL_VERSION + 1
        ));
        assert_eq!(kind, UpdateKind::Hard);
    }

    #[test]
    fn versions_order_by_semver_and_an_older_release_is_not_offered() {
        assert!(is_newer("0.2.0", "0.1.9").expect("valid"));
        assert!(!is_newer("0.1.9", "0.1.9").expect("valid"));
        assert!(!is_newer("0.1.0", "0.1.9").expect("valid"));
        assert!(is_newer("1.0.0-beta.2", "1.0.0-beta.1").expect("valid"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2").expect("valid"));
        assert!(!is_newer("9.0.0", "10.0.0").expect("valid"));
        assert!(is_newer("nope", "0.1.9").is_err());
    }

    #[test]
    fn the_manifest_is_parsed_for_this_platform() {
        let release = parse_release(SAMPLE)
            .expect("the sample parses")
            .expect("newer than this build");
        assert_eq!(release.version, "9.8.7");
        let platform = platform_keys()[0];
        assert_eq!(release.platform, platform);
        assert_eq!(release.kind, UpdateKind::Soft);
        assert_eq!(release.notes, "A softer landing.");
        assert!(release.url.ends_with("app.tar.gz"));
    }

    #[test]
    fn an_http_url_is_refused_before_anything_is_downloaded() {
        let body = SAMPLE.replace("https://github.com", "http://github.com");
        assert!(
            parse_release(&body)
                .expect_err("http is refused")
                .contains("HTTPS")
        );
    }

    #[test]
    fn a_release_without_this_platform_is_an_error() {
        let body = SAMPLE
            .replace("darwin-aarch64", "linux-aarch64")
            .replace("darwin-x86_64", "linux-x86_64");
        assert!(parse_release(&body).is_err());
    }

    #[test]
    fn a_hard_release_is_refused_in_place_with_the_protocols_named() {
        let hard = Release {
            version: "9.8.7".into(),
            notes: String::new(),
            kind: UpdateKind::Hard,
            platform: "darwin-aarch64",
            url: "https://example.invalid/app.tar.gz".into(),
            signature: "sig".into(),
        };
        let message = hard_refusal(&hard).expect("hard is refused");
        assert!(message.contains("daemon protocol"));
        assert!(message.contains(&client::PROTOCOL_VERSION.to_string()));
        assert!(message.contains("full installer"));
        let soft = Release {
            kind: UpdateKind::Soft,
            ..hard
        };
        assert_eq!(hard_refusal(&soft), None);
    }

    #[test]
    fn the_embedded_key_is_a_minisign_key() {
        updater_key().expect("the compiled-in key decodes");
    }

    fn state() -> AppState {
        let store = crate::connection::fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        AppState::for_bench(
            store,
            std::sync::Arc::new(crate::connection::Metrics::default()),
            crate::terminal::SharedTerminal::new(),
        )
    }

    #[test]
    fn installing_a_hard_release_never_reaches_the_worker() {
        let mut s = state();
        s.updates.pending = Some(Release {
            version: "9.8.7".into(),
            notes: String::new(),
            kind: UpdateKind::Hard,
            platform: "darwin-aarch64",
            url: "https://example.invalid/app.tar.gz".into(),
            signature: "sig".into(),
        });
        s.install_update();
        assert!(!s.updates.working);
        assert!(matches!(
            s.updates.state,
            UpdateState::Failed { ref message } if message.contains("full installer")
        ));
    }

    #[test]
    fn a_fixture_check_answers_without_a_network() {
        let mut s = state();
        s.updates.fixture = true;
        s.check_for_updates(true);
        assert_eq!(s.updates.state, UpdateState::UpToDate);
        assert!(s.updates.last_checked.is_some());
    }

    #[test]
    fn tar_paths_may_not_leave_the_staging_folder() {
        assert!(safe_relative(Path::new(
            "Forge Node.app/Contents/Info.plist"
        )));
        assert!(safe_relative(Path::new("./Forge Node.app")));
        assert!(!safe_relative(Path::new("../evil")));
        assert!(!safe_relative(Path::new("/Applications/Forge Node.app")));
        assert!(!safe_relative(Path::new("")));
    }
}
