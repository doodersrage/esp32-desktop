//! ESP-IDF builds of local projects: folders the user opened in the app.
//!
//! A local project is an ESP-IDF project directory (a `CMakeLists.txt` that includes
//! ESP-IDF's `project.cmake`). The user chose it, so unlike the library's downloaded
//! projects it builds without a consent prompt (rule 4 covers downloaded projects).
//! Its id is `local-` and a hash of its path, so it passes [`crate::safe_id`] and
//! fits where library ids go: builds land in `<builds>/<id>/<board>/out/` with the
//! same file names as Arduino builds, and flashing, the simulator and the debugger
//! find them there. The board is one of the generic chip boards (`esp32`,
//! `esp32s3`, `esp32c3`), which set ESP-IDF's target.
//!
//! The build runs `idf.py` from an ESP-IDF install found by [`detect`], in that
//! install's environment ([`Activation`]): on Linux and macOS through its `export.sh`,
//! on Windows, where there is no POSIX shell, the way ESP-IDF's own activation does it
//! (`idf_tools.py export` run by ESP-IDF's Python, then `idf.py` with that
//! environment). Its build directory and `sdkconfig` are in the cache, so the project
//! folder is not written to. Every argument is passed as is: the shell script is fixed
//! text, and paths go in as its positional parameters.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::{Error, Output, Stream, hex, run_logged};

/// Overrides where ESP-IDF is looked for, as ESP-IDF's own tools do.
pub const IDF_PATH_ENV: &str = "IDF_PATH";

/// The boards a local project builds for, and ESP-IDF's target for each.
pub const TARGETS: [(&str, &str); 3] = [
    ("esp32", "esp32"),
    ("esp32s3", "esp32s3"),
    ("esp32c3", "esp32c3"),
];

/// An ESP-IDF install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Idf {
    pub path: PathBuf,
    /// `6.0.2`, from `tools/cmake/version.cmake`.
    pub version: String,
}

/// ESP-IDF: `$IDF_PATH`, else what Espressif's installation manager (EIM) recorded,
/// its selected install first, else `~/esp/esp-idf` (Espressif's guide), else on
/// Windows the classic installer's `C:\Espressif\frameworks\esp-idf-v*` (newest
/// first), else `/opt/esp-idf` (the AUR package). None if none is an install ([`at`]).
pub fn detect() -> Option<Idf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os(IDF_PATH_ENV).filter(|p| !p.is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    candidates.extend(eim_installs().into_iter().map(|i| i.path));
    let home = esp32bench_distro::dirs::home();
    if let Some(home) = &home {
        candidates.push(home.join("esp").join("esp-idf"));
    }
    if cfg!(windows) {
        let mut classic: Vec<PathBuf> = std::fs::read_dir(r"C:\Espressif\frameworks")
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("esp-idf-v"))
            })
            .collect();
        classic.sort_by_key(|p| std::cmp::Reverse(version_key(&p.to_string_lossy())));
        candidates.extend(classic);
    } else {
        candidates.push(PathBuf::from("/opt/esp-idf"));
    }
    candidates.into_iter().find_map(|p| at(&p))
}

/// The install at `path`, if it is one: `tools/idf.py` and `tools/idf_tools.py`, and
/// on Linux and macOS the `export.sh` builds go through.
pub fn at(path: &Path) -> Option<Idf> {
    let tools = path.join("tools");
    if !tools.join("idf.py").is_file()
        || !tools.join("idf_tools.py").is_file()
        || (Activation::platform() == Activation::ExportSh && !path.join("export.sh").is_file())
    {
        return None;
    }
    let text = std::fs::read_to_string(path.join("tools/cmake/version.cmake")).ok()?;
    let part = |name: &str| -> Option<String> {
        let key = format!("set(IDF_VERSION_{name} ");
        let rest = &text[text.find(&key)? + key.len()..];
        Some(rest[..rest.find(')')?].trim().to_owned())
    };
    Some(Idf {
        path: path.to_owned(),
        version: format!("{}.{}.{}", part("MAJOR")?, part("MINOR")?, part("PATCH")?),
    })
}

/// An install Espressif's installation manager recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EimInstall {
    pub path: PathBuf,
    /// The Python of its environment, when recorded.
    pub python: Option<PathBuf>,
    /// Where its tools are (`IDF_TOOLS_PATH`), when recorded.
    pub tools: Option<PathBuf>,
}

/// Where EIM keeps its record of installs: `C:\Espressif\tools\eim_idf.json` on
/// Windows, `~/.espressif/tools/eim_idf.json` elsewhere.
fn eim_files() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        v.push(PathBuf::from(r"C:\Espressif\tools\eim_idf.json"));
    }
    if let Some(home) = esp32bench_distro::dirs::home() {
        v.push(home.join(".espressif").join("tools").join("eim_idf.json"));
    }
    v
}

/// The installs EIM recorded, its selected one first.
pub fn eim_installs() -> Vec<EimInstall> {
    eim_files()
        .iter()
        .filter_map(|f| std::fs::read(f).ok())
        .flat_map(|bytes| parse_eim(&bytes))
        .collect()
}

/// Read `eim_idf.json`: `idfInstalled` is a list of installs (`id`, `path`, `python`,
/// `idfToolsPath`), `idfSelectedId` the one in use. Unknown or missing fields are
/// skipped, so a newer EIM's file still gives what it can.
fn parse_eim(bytes: &[u8]) -> Vec<EimInstall> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let selected = v.get("idfSelectedId").and_then(|s| s.as_str());
    let path = |o: &serde_json::Value, k: &str| {
        o.get(k)
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    let mut all: Vec<(bool, EimInstall)> = v
        .get("idfInstalled")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let is_selected =
                selected.is_some() && o.get("id").and_then(|s| s.as_str()) == selected;
            Some((
                is_selected,
                EimInstall {
                    path: path(o, "path")?,
                    python: path(o, "python"),
                    tools: path(o, "idfToolsPath"),
                },
            ))
        })
        .collect();
    all.sort_by_key(|(sel, _)| !*sel);
    all.into_iter().map(|(_, i)| i).collect()
}

/// Version numbers in `s` (`esp-idf-v5.4.1` → [5, 4, 1]), to sort installs newest first.
fn version_key(s: &str) -> Vec<u32> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect()
}

/// How a build gets ESP-IDF's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// `bash`, sourcing the install's `export.sh` (Linux and macOS).
    ExportSh,
    /// ESP-IDF's Python runs `idf_tools.py export` and then `idf.py` with what it
    /// printed, with no shell (Windows; usable anywhere the install has its Python).
    Direct,
}

impl Activation {
    pub fn platform() -> Self {
        if cfg!(windows) {
            Activation::Direct
        } else {
            Activation::ExportSh
        }
    }
}

/// ESP-IDF's environment for [`Activation::Direct`]: its Python and the variables
/// `idf_tools.py export` gives.
#[derive(Debug, Clone)]
pub struct Environment {
    pub python: PathBuf,
    pub vars: Vec<(String, OsString)>,
}

/// The Python in a virtual environment at `env`.
fn venv_python(env: &Path) -> Option<PathBuf> {
    [
        env.join("Scripts").join("python.exe"),
        env.join("bin").join("python"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// ESP-IDF's Python environment for `idf` and the tools folder it is in:
/// `$IDF_PYTHON_ENV_PATH`, else what EIM recorded for this install, else
/// `<tools>/python_env/idf<major>.<minor>_py*_env` (the newest Python) in
/// `$IDF_TOOLS_PATH`, `~/.espressif` or, on Windows, `C:\Espressif`.
pub fn python_env(idf: &Idf) -> Option<(PathBuf, Option<PathBuf>)> {
    let tools_env = std::env::var_os("IDF_TOOLS_PATH")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    if let Some(env) = std::env::var_os("IDF_PYTHON_ENV_PATH").filter(|p| !p.is_empty()) {
        return venv_python(Path::new(&env)).map(|p| (p, tools_env));
    }
    let eim = eim_installs().into_iter().find(|i| i.path == idf.path);
    if let Some(i) = &eim
        && let Some(py) = i.python.as_ref().filter(|p| p.is_file())
    {
        return Some((py.clone(), i.tools.clone().or(tools_env)));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    roots.extend(tools_env);
    roots.extend(eim.and_then(|i| i.tools));
    if let Some(home) = esp32bench_distro::dirs::home() {
        roots.push(home.join(".espressif"));
    }
    if cfg!(windows) {
        roots.push(PathBuf::from(r"C:\Espressif"));
    }
    let (major_minor, _) = idf.version.rsplit_once('.')?;
    let prefix = format!("idf{major_minor}_py");
    roots.into_iter().find_map(|root| {
        let mut envs: Vec<PathBuf> = std::fs::read_dir(root.join("python_env"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
            })
            .collect();
        envs.sort_by_key(|p| std::cmp::Reverse(version_key(&p.to_string_lossy())));
        envs.iter()
            .find_map(|e| venv_python(e))
            .map(|py| (py, Some(root.clone())))
    })
}

/// The variables in `idf_tools.py export --format key-value` output, with its PATH's
/// `$PATH` or `%PATH%` replaced by `path` (this process's PATH).
fn parse_export(stdout: &str, path: &OsString) -> Vec<(String, OsString)> {
    stdout
        .lines()
        .filter_map(|l| l.trim_end().split_once('='))
        .filter(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .map(|(k, v)| {
            let v = if k == "PATH" {
                let (head, tail) =
                    match v.strip_suffix("%PATH%").or_else(|| v.strip_suffix("$PATH")) {
                        Some(head) => (head, true),
                        None => (v, false),
                    };
                let mut out = OsString::from(head);
                if tail {
                    out.push(path);
                }
                out
            } else {
                OsString::from(v)
            };
            (k.to_owned(), v)
        })
        .collect()
}

/// ESP-IDF's environment, as its activation computes it, for [`Activation::Direct`].
pub async fn environment(idf: &Idf) -> Result<Environment, Error> {
    let (python, tools) = python_env(idf).ok_or_else(|| Error::NoIdfPython(idf.path.clone()))?;
    let env_dir = python
        .parent()
        .and_then(Path::parent)
        .map(Path::to_owned)
        .unwrap_or_default();
    let mut base: Vec<(String, OsString)> = vec![
        (IDF_PATH_ENV.into(), idf.path.clone().into()),
        ("IDF_PYTHON_ENV_PATH".into(), env_dir.into()),
    ];
    if let Some(t) = tools {
        base.push(("IDF_TOOLS_PATH".into(), t.into()));
    }
    let mut cmd = Command::new(&python);
    cmd.arg(idf.path.join("tools").join("idf_tools.py"))
        .args(["export", "--format", "key-value"])
        .envs(base.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(Error::IdfExport(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut vars = parse_export(&String::from_utf8_lossy(&out.stdout), &path);
    // The variables idf_tools.py leaves out when they are already right.
    for (k, v) in base {
        if !vars.iter().any(|(n, _)| *n == k) {
            vars.push((k, v));
        }
    }
    if let Some((major_minor, _)) = idf.version.rsplit_once('.')
        && !vars.iter().any(|(n, _)| n == "ESP_IDF_VERSION")
    {
        vars.push(("ESP_IDF_VERSION".into(), major_minor.into()));
    }
    Ok(Environment { python, vars })
}

/// A local ESP-IDF project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProject {
    pub id: String,
    /// The project's name, from `project(<name>)` in its `CMakeLists.txt`.
    pub name: String,
    pub path: PathBuf,
}

/// The id of the project at `path` (canonical): `local-` and 12 hex digits.
pub fn local_id(path: &Path) -> String {
    let digest = Sha256::digest(path.as_os_str().as_encoded_bytes());
    format!("local-{}", &hex(&digest)[..12])
}

/// Whether an id names a local project rather than a library one.
pub fn is_local(id: &str) -> bool {
    id.starts_with("local-")
}

/// Check that `path` is an ESP-IDF project and describe it.
pub fn check_project(path: &Path) -> Result<LocalProject, Error> {
    let path = path
        .canonicalize()
        .map_err(|e| Error::NotAnIdfProject(format!("{}: {e}", path.display())))?;
    let cmake = path.join("CMakeLists.txt");
    let text = std::fs::read_to_string(&cmake).map_err(|_| {
        Error::NotAnIdfProject(format!(
            "{} has no CMakeLists.txt: it is not an ESP-IDF project",
            path.display()
        ))
    })?;
    if !text.contains("tools/cmake/project.cmake") {
        return Err(Error::NotAnIdfProject(format!(
            "{} does not include ESP-IDF's project.cmake: it is not an ESP-IDF project",
            cmake.display()
        )));
    }
    let name = project_name(&text).unwrap_or_else(|| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    Ok(LocalProject {
        id: local_id(&path),
        name,
        path,
    })
}

/// `project(<name>)`'s name, the CMake way (first argument).
fn project_name(cmake: &str) -> Option<String> {
    cmake.lines().map(str::trim).find_map(|l| {
        let rest = l.strip_prefix("project(")?;
        let name = rest.split([' ', ')', '\t']).next()?.trim();
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// The local projects opened so far, in a JSON file (in `$XDG_DATA_HOME/esp32bench`).
#[derive(Debug, Clone)]
pub struct Registry {
    file: PathBuf,
}

impl Registry {
    pub fn new(file: PathBuf) -> Self {
        Registry { file }
    }

    pub fn list(&self) -> Vec<LocalProject> {
        std::fs::read(&self.file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<LocalProject> {
        self.list().into_iter().find(|p| p.id == id)
    }

    /// Add (or refresh) a project; returns it.
    pub fn add(&self, path: &Path) -> Result<LocalProject, Error> {
        let p = check_project(path)?;
        let mut all = self.list();
        all.retain(|x| x.id != p.id);
        all.push(p.clone());
        all.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        self.save(&all)?;
        Ok(p)
    }

    /// Forget a project (its folder is not touched). Returns whether it was there.
    pub fn remove(&self, id: &str) -> Result<bool, Error> {
        let mut all = self.list();
        let before = all.len();
        all.retain(|x| x.id != id);
        self.save(&all)?;
        Ok(all.len() != before)
    }

    fn save(&self, all: &[LocalProject]) -> Result<(), Error> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(all).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, &self.file)?;
        Ok(())
    }
}

/// An ESP-IDF build of a local project for one board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub project: String,
    pub board: String,
    /// ESP-IDF's target.
    pub target: String,
    pub path: PathBuf,
    pub idf: Idf,
}

/// Work out a build of `project` for `board` without running anything.
pub fn plan(project: &LocalProject, board: &str, idf: &Idf) -> Result<Plan, Error> {
    let target = TARGETS
        .iter()
        .find(|(b, _)| *b == board)
        .map(|(_, t)| (*t).to_owned())
        .ok_or_else(|| Error::UnknownBoard(board.to_owned()))?;
    Ok(Plan {
        project: project.id.clone(),
        board: board.to_owned(),
        target,
        path: project.path.clone(),
        idf: idf.clone(),
    })
}

/// The directories one build uses, under `root/<project>/<board>/`.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub build: PathBuf,
    pub out: PathBuf,
    pub sdkconfig: PathBuf,
}

impl Dirs {
    pub fn new(root: &Path, plan: &Plan) -> Self {
        let base = root.join(&plan.project).join(&plan.board);
        Dirs {
            build: base.join("idf-build"),
            out: base.join("out"),
            sdkconfig: base.join("sdkconfig"),
        }
    }
}

/// `idf.py` with ESP-IDF's environment. Through `export.sh`:
/// `bash -c <script> esp32bench-idf <idf> <args>`, the script fixed and ESP-IDF's path
/// and idf.py's arguments its parameters. Direct: `<python> <idf>/tools/idf.py <args>`
/// with the environment's variables.
fn idf_py(idf: &Idf, env: Option<&Environment>, args: Vec<OsString>) -> Command {
    if let Some(env) = env {
        let mut cmd = Command::new(&env.python);
        cmd.arg(idf.path.join("tools").join("idf.py"))
            .args(args)
            .envs(env.vars.iter().map(|(k, v)| (k, v)));
        return cmd;
    }
    const SCRIPT: &str = r#"set -e
idf="$1"; shift
args=("$@"); set --
. "$idf/export.sh" > /dev/null
exec idf.py "${args[@]}""#;
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(SCRIPT)
        .arg("esp32bench-idf")
        .arg(&idf.path)
        .args(args);
    cmd
}

/// idf.py's project, build directory and sdkconfig options for a plan.
pub fn common_args(plan: &Plan, dirs: &Dirs) -> Vec<OsString> {
    let mut sdkconfig = OsString::from("SDKCONFIG=");
    sdkconfig.push(&dirs.sdkconfig);
    vec![
        "-C".into(),
        plan.path.clone().into(),
        "-B".into(),
        dirs.build.clone().into(),
        "-D".into(),
        sdkconfig,
        "-D".into(),
        format!("IDF_TARGET={}", plan.target).into(),
    ]
}

/// Where a build's merged image and ELF go: the names Arduino builds use, so flashing,
/// the simulator and the debugger find them.
pub fn outputs(plan: &Plan, dirs: &Dirs) -> Output {
    Output {
        image: dirs.out.join(format!("{}.ino.merged.bin", plan.project)),
        elf: dirs.out.join(format!("{}.ino.elf", plan.project)),
    }
}

/// The flash size to pad the merged image to: the project's own (`flash_size` in the
/// build's flasher_args.json) when QEMU can emulate it, else 4 MB. A 2 MB project
/// then boots on 2 MB of simulated flash, without ESP-IDF's warning that the chip's
/// flash is larger than the image says.
fn image_size(flasher_args: &[u8]) -> &'static str {
    let size = serde_json::from_slice::<serde_json::Value>(flasher_args)
        .ok()
        .and_then(|v| {
            v.pointer("/flash_settings/flash_size")
                .and_then(|s| s.as_str())
                .map(str::to_owned)
        });
    match size.as_deref() {
        Some("2MB") => "2MB",
        Some("8MB") => "8MB",
        Some("16MB") => "16MB",
        _ => "4MB",
    }
}

/// Build: `idf.py build`, then `merge-bin` into one flash image of the project's
/// flash size (`image_size`), and the ELF copied beside it. Every line goes to `log`;
/// dropping the future kills the build.
pub async fn run(
    plan: &Plan,
    dirs: &Dirs,
    log: impl FnMut(Stream, String),
) -> Result<Output, Error> {
    run_with(plan, dirs, Activation::platform(), log).await
}

/// [`run`] with the environment got as `activation` says.
pub async fn run_with(
    plan: &Plan,
    dirs: &Dirs,
    activation: Activation,
    mut log: impl FnMut(Stream, String),
) -> Result<Output, Error> {
    tokio::fs::create_dir_all(&dirs.build).await?;
    tokio::fs::create_dir_all(&dirs.out).await?;
    let out = outputs(plan, dirs);
    let env = match activation {
        Activation::ExportSh => None,
        Activation::Direct => Some(environment(&plan.idf).await?),
    };
    let env = env.as_ref();

    let mut args = common_args(plan, dirs);
    args.push("build".into());
    run_logged(idf_py(&plan.idf, env, args), "idf.py build", &mut log).await?;

    let size = image_size(
        &tokio::fs::read(dirs.build.join("flasher_args.json"))
            .await
            .unwrap_or_default(),
    );
    let mut args = common_args(plan, dirs);
    args.extend([
        "merge-bin".into(),
        "-o".into(),
        out.image.clone().into(),
        "-f".into(),
        "raw".into(),
        "--pad-to-size".into(),
        size.into(),
    ]);
    run_logged(idf_py(&plan.idf, env, args), "idf.py merge-bin", &mut log).await?;

    // The ELF's name is the project's; ESP-IDF records it in project_description.json.
    let desc: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(dirs.build.join("project_description.json")).await?,
    )
    .map_err(std::io::Error::other)?;
    let elf = desc
        .get("app_elf")
        .and_then(|v| v.as_str())
        .map(|n| dirs.build.join(n))
        .ok_or_else(|| Error::MissingOutput(dirs.build.join("<app>.elf")))?;
    tokio::fs::copy(&elf, &out.elf).await?;
    for p in [&out.image, &out.elf] {
        if !p.is_file() {
            return Err(Error::MissingOutput(p.clone()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(dir: &Path, cmake: &str) {
        std::fs::create_dir_all(dir.join("main")).unwrap();
        std::fs::write(dir.join("CMakeLists.txt"), cmake).unwrap();
    }

    const CMAKE: &str = "cmake_minimum_required(VERSION 3.16)\ninclude($ENV{IDF_PATH}/tools/cmake/project.cmake)\nproject(blinky)\n";

    #[test]
    fn images_take_the_projects_flash_size_when_qemu_has_it() {
        let args = |size: &str| format!(r#"{{"flash_settings": {{"flash_size": "{size}"}}}}"#);
        assert_eq!(image_size(args("2MB").as_bytes()), "2MB");
        assert_eq!(image_size(args("16MB").as_bytes()), "16MB");
        // QEMU has no 1 or 32 MB flash; "keep" and missing files get 4 MB.
        assert_eq!(image_size(args("1MB").as_bytes()), "4MB");
        assert_eq!(image_size(args("32MB").as_bytes()), "4MB");
        assert_eq!(image_size(args("keep").as_bytes()), "4MB");
        assert_eq!(image_size(b""), "4MB");
    }

    #[test]
    fn opens_idf_projects_and_refuses_other_folders() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("blinky");
        project(&p, CMAKE);
        let lp = check_project(&p).unwrap();
        assert_eq!(lp.name, "blinky");
        assert!(is_local(&lp.id));
        crate::safe_id(&lp.id).unwrap();
        assert_eq!(lp.id, local_id(&p.canonicalize().unwrap()));

        let other = t.path().join("other");
        project(&other, "project(x)\n");
        assert!(matches!(
            check_project(&other),
            Err(Error::NotAnIdfProject(_))
        ));
        assert!(matches!(
            check_project(&t.path().join("none")),
            Err(Error::NotAnIdfProject(_))
        ));
    }

    #[test]
    fn registry_adds_lists_and_forgets() {
        let t = tempfile::tempdir().unwrap();
        let r = Registry::new(t.path().join("data/local-projects.json"));
        assert!(r.list().is_empty());
        let p = t.path().join("blinky");
        project(&p, CMAKE);
        let lp = r.add(&p).unwrap();
        // Adding again does not duplicate.
        r.add(&p).unwrap();
        assert_eq!(r.list(), vec![lp.clone()]);
        assert_eq!(r.get(&lp.id), Some(lp.clone()));
        assert!(r.remove(&lp.id).unwrap());
        assert!(!r.remove(&lp.id).unwrap());
        assert!(r.list().is_empty());
        // The folder itself is untouched.
        assert!(p.join("CMakeLists.txt").is_file());
    }

    #[test]
    fn plans_only_the_simulated_chips_and_keeps_the_project_folder_clean() {
        let idf = Idf {
            path: "/opt/esp-idf".into(),
            version: "6.0.2".into(),
        };
        let lp = LocalProject {
            id: "local-0123456789ab".into(),
            name: "blinky".into(),
            path: "/home/u/blinky".into(),
        };
        let p = plan(&lp, "esp32s3", &idf).unwrap();
        assert_eq!(p.target, "esp32s3");
        assert!(matches!(
            plan(&lp, "esp32-cam", &idf),
            Err(Error::UnknownBoard(_))
        ));
        let dirs = Dirs::new(Path::new("/c/builds"), &p);
        let args: Vec<String> = common_args(&p, &dirs)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "-C",
                "/home/u/blinky",
                "-B",
                "/c/builds/local-0123456789ab/esp32s3/idf-build",
                "-D",
                "SDKCONFIG=/c/builds/local-0123456789ab/esp32s3/sdkconfig",
                "-D",
                "IDF_TARGET=esp32s3"
            ]
        );
        let out = outputs(&p, &dirs);
        assert_eq!(
            out.image,
            Path::new("/c/builds/local-0123456789ab/esp32s3/out/local-0123456789ab.ino.merged.bin")
        );
    }

    #[test]
    fn reads_the_installed_version() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("tools/cmake")).unwrap();
        std::fs::write(t.path().join("export.sh"), "").unwrap();
        std::fs::write(t.path().join("tools/idf.py"), "").unwrap();
        std::fs::write(t.path().join("tools/idf_tools.py"), "").unwrap();
        std::fs::write(
            t.path().join("tools/cmake/version.cmake"),
            "set(IDF_VERSION_MAJOR 6)\nset(IDF_VERSION_MINOR 0)\nset(IDF_VERSION_PATCH 2)\n",
        )
        .unwrap();
        assert_eq!(at(t.path()).unwrap().version, "6.0.2");
        assert!(at(&t.path().join("nope")).is_none());
    }

    #[test]
    fn reads_eims_record_selected_install_first() {
        let json = br#"{
            "eimPath": "C:\\Espressif\\eim.exe",
            "idfSelectedId": "esp-idf-b",
            "idfInstalled": [
                {"id": "esp-idf-a", "name": "v5.4.1", "path": "C:\\Espressif\\v5.4.1\\esp-idf",
                 "python": "C:\\Espressif\\tools\\python\\v5.4.1\\venv\\Scripts\\python.exe",
                 "idfToolsPath": "C:\\Espressif\\tools"},
                {"id": "esp-idf-b", "name": "v6.0.2", "path": "C:\\Espressif\\v6.0.2\\esp-idf"},
                {"id": "broken"}
            ]
        }"#;
        let all = parse_eim(json);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].path, PathBuf::from(r"C:\Espressif\v6.0.2\esp-idf"));
        assert_eq!(all[0].python, None);
        assert_eq!(all[1].tools, Some(PathBuf::from(r"C:\Espressif\tools")));
        assert!(parse_eim(b"not json").is_empty());
    }

    #[test]
    fn export_output_becomes_variables_with_this_path() {
        let path = OsString::from("/usr/bin");
        let unix = parse_export(
            "IDF_PYTHON_ENV_PATH=/e/py\nPATH=/t/bin:/idf/tools:$PATH\nnoise line\n",
            &path,
        );
        assert_eq!(
            unix,
            vec![
                ("IDF_PYTHON_ENV_PATH".to_owned(), OsString::from("/e/py")),
                (
                    "PATH".to_owned(),
                    OsString::from("/t/bin:/idf/tools:/usr/bin")
                ),
            ]
        );
        let win = parse_export("PATH=C:\\t\\bin;%PATH%\r\nESP_IDF_VERSION=6.0\r\n", &path);
        assert_eq!(win[0].1, OsString::from("C:\\t\\bin;/usr/bin"));
        assert_eq!(win[1].1, OsString::from("6.0"));
    }

    #[test]
    fn sorts_versions_numerically() {
        let mut v = ["idf6.0_py3.9_env", "idf6.0_py3.14_env", "idf6.0_py3.11_env"];
        v.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
        assert_eq!(v[0], "idf6.0_py3.14_env");
    }
}
