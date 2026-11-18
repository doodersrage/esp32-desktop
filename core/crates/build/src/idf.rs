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
//! The build runs `idf.py` from an ESP-IDF install found by [`detect`], through that
//! install's `export.sh`. Its build directory and `sdkconfig` are in the cache, so the
//! project folder is not written to. Every argument is passed as is: the shell script
//! is fixed text, and paths go in as its positional parameters.

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

/// ESP-IDF: `$IDF_PATH`, else `~/esp/esp-idf` (Espressif's guide), else
/// `/opt/esp-idf` (the AUR package). None if none has `export.sh` and `tools/idf.py`.
pub fn detect() -> Option<Idf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os(IDF_PATH_ENV).filter(|p| !p.is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("esp/esp-idf"));
    }
    candidates.push(PathBuf::from("/opt/esp-idf"));
    candidates.into_iter().find_map(|p| at(&p))
}

/// The install at `path`, if it is one.
pub fn at(path: &Path) -> Option<Idf> {
    if !path.join("export.sh").is_file() || !path.join("tools/idf.py").is_file() {
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

/// `idf.py` with ESP-IDF's environment: `bash -c <script> esp32bench-idf <idf> <args>`.
/// The script is fixed; ESP-IDF's path and idf.py's arguments are its parameters.
fn idf_py(idf: &Idf, args: Vec<OsString>) -> Command {
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

/// Build: `idf.py build`, then `merge-bin` into one 4 MB flash image, and the ELF
/// copied beside it. Every line goes to `log`; dropping the future kills the build.
pub async fn run(
    plan: &Plan,
    dirs: &Dirs,
    mut log: impl FnMut(Stream, String),
) -> Result<Output, Error> {
    tokio::fs::create_dir_all(&dirs.build).await?;
    tokio::fs::create_dir_all(&dirs.out).await?;
    let out = outputs(plan, dirs);

    let mut args = common_args(plan, dirs);
    args.push("build".into());
    run_logged(idf_py(&plan.idf, args), "idf.py build", &mut log).await?;

    let mut args = common_args(plan, dirs);
    args.extend([
        "merge-bin".into(),
        "-o".into(),
        out.image.clone().into(),
        "-f".into(),
        "raw".into(),
        "--pad-to-size".into(),
        "4MB".into(),
    ]);
    run_logged(idf_py(&plan.idf, args), "idf.py merge-bin", &mut log).await?;

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
        std::fs::write(
            t.path().join("tools/cmake/version.cmake"),
            "set(IDF_VERSION_MAJOR 6)\nset(IDF_VERSION_MINOR 0)\nset(IDF_VERSION_PATCH 2)\n",
        )
        .unwrap();
        assert_eq!(at(t.path()).unwrap().version, "6.0.2");
        assert!(at(&t.path().join("nope")).is_none());
    }
}
