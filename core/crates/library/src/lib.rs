//! The esp32bench.dev project library.
//!
//! The site publishes `/desktop-manifest.json` (format in
//! `schema/project-manifest/v1.schema.json`). This crate fetches it, checks the format
//! and major version, caches it for offline use and turns it into the summaries the
//! UI lists. Reading is lenient: unknown fields are ignored, because a 1.x manifest
//! may add optional fields.

pub mod link;

pub mod expect;
pub mod explain;
pub mod starred;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

pub const FORMAT: &str = "esp32bench-project-manifest";
/// The manifest major version this code reads.
pub const MAJOR: u64 = 1;
pub const DEFAULT_SOURCE: &str = "https://esp32bench.dev/desktop-manifest.json";
/// Overrides the source: an https URL or a local file path (e.g. a site build's dist/).
pub const SOURCE_ENV: &str = "ESP32BENCH_MANIFEST";
const MAX_BYTES: usize = 32 * 1024 * 1024;
const CACHE_FILE: &str = "desktop-manifest.json";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a project manifest: {0}")]
    Format(String),
    #[error("manifest version {found} is not supported; this program reads {MAJOR}.x")]
    Version { found: String },
    #[error("download of {from} failed: {detail}")]
    Download { from: String, detail: String },
    #[error("manifest is larger than {MAX_BYTES} bytes")]
    TooLarge,
    #[error("no cache directory: set XDG_CACHE_HOME or HOME")]
    NoCacheDir,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub version: String,
    pub generated: String,
    pub site: String,
    pub licence: Licence,
    pub boards: Vec<Board>,
    pub projects: Vec<Project>,
    /// What to tell the user about lines of output (manifest 1.1): the site's crash,
    /// reset and upload explanations, applied by [`explain::Explainer`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explain: Vec<explain::Rule>,
    /// The desktop app's latest release (manifest 1.2), so older apps can say a newer
    /// one is out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<Desktop>,
}

/// The desktop app's latest release, as the site lists it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Desktop {
    /// Its version, `X.Y.Z`.
    pub version: String,
    /// The page to get it from (https).
    pub url: String,
}

impl Manifest {
    /// The site's latest desktop release, if it is newer than `current` (the running
    /// core's version) and its page is https.
    pub fn newer_desktop(&self, current: &str) -> Option<&Desktop> {
        let d = self.desktop.as_ref()?;
        (d.url.starts_with("https://") && is_newer(&d.version, current)).then_some(d)
    }
}

/// Whether version `a` (`X.Y.Z`) is newer than `b`; false if either does not parse.
/// A pre-release suffix (`-rc1`) is ignored.
pub fn is_newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Option<[u64; 3]> {
        let v = v.trim().split(['-', '+']).next()?;
        let mut it = v.split('.').map(|n| n.parse::<u64>().ok());
        let r = [it.next()??, it.next()??, it.next()??];
        it.next().is_none().then_some(r)
    }
    matches!((parts(a), parts(b)), (Some(a), Some(b)) if a > b)
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Licence {
    pub code: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: String,
    pub name: String,
    pub short: String,
    pub chip: String,
    pub arduino_board: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_as: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub url: String,
    pub level: String,
    pub minutes: u32,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    pub summary: String,
    pub firmware: Firmware,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub libraries: Vec<String>,
    /// Libraries to download before building (not the ones built into the core).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub install: Vec<Install>,
    /// Arduino-ESP32 partition scheme, when the default is too small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partitions: Option<String>,
    pub baud: u32,
    pub needs: Vec<Vec<String>>,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
    pub builds: Vec<Build>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Install {
    pub library: String,
    /// `owner/name`; the Arduino library name is the part after the slash.
    pub registry: String,
}

impl Install {
    /// The name `arduino-cli lib install` takes.
    pub fn arduino_name(&self) -> &str {
        self.registry
            .split_once('/')
            .map_or(self.registry.as_str(), |(_, n)| n)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Firmware {
    Sketch,
    Prebuilt,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Part {
    pub quantity: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Build {
    pub board: String,
    pub pins: Vec<String>,
    pub wiring: Vec<Wire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub sample: Vec<String>,
    /// For prebuilt firmware (manifest 1.1): the merged image to download, written at
    /// offset 0, with its SHA-256 and size so the app can check what it got.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Image {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// An image's URL as the app may fetch it: an https URL, or, when the library itself
/// is a local file (development and tests), a path relative to that file's folder
/// that stays inside it. Anything else (http, other schemes, absolute or escaping
/// paths) is refused, so a manifest cannot make the app read other local files.
pub fn image_source(library_source: &str, url: &str) -> Result<String, Error> {
    if url.starts_with("https://") {
        return Ok(url.to_owned());
    }
    let refused = |detail: &str| Error::Download {
        from: url.to_owned(),
        detail: detail.to_owned(),
    };
    if url.contains("://") || library_source.starts_with("https://") {
        return Err(refused("firmware images are only downloaded over https"));
    }
    let rel = std::path::Path::new(url);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(refused(
            "a local image must be a path inside the library's folder",
        ));
    }
    let dir = std::path::Path::new(library_source)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    Ok(dir.join(rel).to_string_lossy().into_owned())
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Wire {
    pub from: String,
    pub to: String,
    pub note: String,
}

/// One row of the library list (`library.list`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub level: String,
    pub minutes: u32,
    pub tags: Vec<String>,
    pub firmware: Firmware,
    /// Board ids the project supports, in the site's order.
    pub boards: Vec<String>,
    /// Distinct chips of those boards, in board order.
    pub chips: Vec<String>,
    pub url: String,
    /// Whether it runs in the app's simulator: a build for an ESP32, ESP32-S3 or
    /// ESP32-C3 board of a sketch that does not use Bluetooth, which the simulator
    /// has no radio for. The library sweep (docs/library-sweep.md) is what says so.
    pub simulator: bool,
}

/// Chips the simulator runs.
pub const SIMULATED_CHIPS: &[&str] = &["esp32", "esp32s3", "esp32c3"];

/// Whether a project uses Bluetooth: a BLE or Bluetooth library, or a sketch that
/// includes one of their headers.
pub fn uses_bluetooth(p: &Project) -> bool {
    let named = |s: &str| {
        let l = s.to_lowercase();
        l.contains("ble") || l.contains("bluetooth") || l.contains("esp_bt")
    };
    p.libraries.iter().any(|l| named(l))
        || p.builds
            .iter()
            .filter_map(|b| b.code.as_deref())
            .any(|code| {
                code.lines()
                    .filter(|l| l.trim_start().starts_with("#include"))
                    .any(named)
            })
}

/// Parse and check a manifest: right format, a major version this code reads.
pub fn parse(bytes: &[u8]) -> Result<Manifest, Error> {
    // Check format and version first, so an unrelated or newer file gets a clear error
    // instead of a field-level one.
    #[derive(Deserialize)]
    struct Head {
        format: Option<String>,
        version: Option<String>,
    }
    let head: Head = serde_json::from_slice(bytes).map_err(|e| Error::Format(e.to_string()))?;
    if head.format.as_deref() != Some(FORMAT) {
        return Err(Error::Format(format!(
            "format is {:?}, expected {FORMAT:?}",
            head.format.unwrap_or_default()
        )));
    }
    let version = head.version.unwrap_or_default();
    let major = version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u64>().ok());
    if major != Some(MAJOR) {
        return Err(Error::Version { found: version });
    }
    serde_json::from_slice(bytes).map_err(|e| Error::Format(e.to_string()))
}

impl Manifest {
    pub fn summaries(&self) -> Vec<Summary> {
        self.projects
            .iter()
            .map(|p| {
                let boards: Vec<String> = p.builds.iter().map(|b| b.board.clone()).collect();
                let mut chips: Vec<String> = Vec::new();
                for b in &boards {
                    if let Some(chip) = self.boards.iter().find(|x| &x.id == b).map(|x| &x.chip)
                        && !chips.contains(chip)
                    {
                        chips.push(chip.clone());
                    }
                }
                Summary {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    summary: p.summary.clone(),
                    level: p.level.clone(),
                    minutes: p.minutes,
                    tags: p.tags.clone(),
                    firmware: p.firmware,
                    boards,
                    simulator: (p.firmware == Firmware::Sketch
                        || p.builds.iter().any(|b| b.image.is_some()))
                        && chips.iter().any(|c| SIMULATED_CHIPS.contains(&c.as_str()))
                        && !uses_bluetooth(p),
                    chips,
                    url: p.url.clone(),
                }
            })
            .collect()
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }
}

/// Where to fetch from: `$ESP32BENCH_MANIFEST` if set, else the live site.
pub fn default_source() -> String {
    std::env::var(SOURCE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SOURCE.to_owned())
}

/// Fetch manifest bytes from an https URL (with curl) or a local file path.
pub async fn fetch(source: &str) -> Result<Vec<u8>, Error> {
    let bytes = if source.starts_with("https://") {
        let max = MAX_BYTES.to_string();
        let out = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--compressed",
            ])
            .args(["--proto", "=https", "--proto-redir", "=https"])
            .args(["--max-time", "60", "--max-filesize", &max])
            .arg(source)
            .kill_on_drop(true)
            .output()
            .await?;
        if !out.status.success() {
            return Err(Error::Download {
                from: source.to_owned(),
                detail: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        out.stdout
    } else if source.contains("://") {
        return Err(Error::Download {
            from: source.to_owned(),
            detail: "only https URLs or local file paths are allowed".into(),
        });
    } else {
        tokio::fs::read(source).await?
    };
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

/// The app's cache directory (`esp32bench_distro::dirs::cache_dir`):
/// `$XDG_CACHE_HOME/esp32bench`, `~/.cache/esp32bench`, or the system's own place.
pub fn cache_dir() -> Result<PathBuf, Error> {
    esp32bench_distro::dirs::cache_dir().ok_or(Error::NoCacheDir)
}

/// The library as last fetched, from `dir`.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Cache { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(CACHE_FILE)
    }

    /// The cached manifest, or None if nothing has been fetched yet.
    pub async fn load(&self) -> Result<Option<Manifest>, Error> {
        match tokio::fs::read(self.path()).await {
            Ok(b) => parse(&b).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Check `bytes` and store them. A bad download never replaces a good cache.
    pub async fn store(&self, bytes: &[u8]) -> Result<Manifest, Error> {
        let manifest = parse(bytes)?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let tmp = self.dir.join(format!(".{CACHE_FILE}.partial"));
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, self.path()).await?;
        Ok(manifest)
    }

    /// Fetch from `source`, check and store.
    pub async fn refresh(&self, source: &str) -> Result<Manifest, Error> {
        let bytes = fetch(source).await?;
        self.store(&bytes).await
    }
}

/// When the cache file was last written, as seconds since the Unix epoch.
pub fn modified_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}
