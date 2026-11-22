//! The user's own Arduino sketches (phase 7): a folder holding `<folder>.ino`, as the
//! Arduino IDE keeps them, built in place with arduino-cli for a board from the
//! library. The user opened the folder, so no consent is asked (rule 4 is about
//! downloaded projects). The outputs are copied to the names the rest of the core
//! expects (`<project id>.ino.merged.bin`), so the simulator, debugger and flashing
//! treat them like any other build.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{BOARDS, Error, Output, Stream, Toolchain, step};

/// The sketch's main file in `folder`: `<folder name>.ino`, if it is there.
pub fn main_file(folder: &Path) -> Option<PathBuf> {
    let name = folder.file_name()?;
    let ino = folder.join(format!("{}.ino", name.to_string_lossy()));
    ino.is_file().then_some(ino)
}

/// The sketch folder `path` names: the folder itself, or the folder of a `.ino` file
/// in it, canonical; `None` if it is not an Arduino sketch.
pub fn folder(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    let dir = if path.is_file() {
        path.parent()?.to_path_buf()
    } else {
        path
    };
    main_file(&dir).map(|_| dir)
}

/// Its source files, relative to the folder: `.ino`, `.cpp`, `.c`, `.h` and `.hpp`
/// at its top and in `src/` (what arduino-cli compiles), sorted.
pub fn sources(folder: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for sub in ["", "src"] {
        let Ok(entries) = std::fs::read_dir(folder.join(sub)) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let known = p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x, "ino" | "cpp" | "c" | "h" | "hpp"));
            if known && p.is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                out.push(if sub.is_empty() {
                    name
                } else {
                    format!("{sub}/{name}")
                });
            }
        }
    }
    out.sort();
    out
}

/// The boards a sketch can be built for: those the library lists that this program
/// knows a board package for, as `(board id, fqbn base)`.
pub fn boards() -> impl Iterator<Item = (&'static str, &'static str)> {
    BOARDS.iter().map(|(id, base, _)| (*id, *base))
}

/// A build of the user's sketch for one board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The local project's id.
    pub project: String,
    pub board: String,
    pub fqbn: String,
    /// The sketch folder, built in place.
    pub path: PathBuf,
    /// The user's own Arduino libraries (`~/Arduino/libraries`), when there are some.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraries: Option<PathBuf>,
}

/// Plan a build of the sketch in `path` (local project `project`) for `board`, whose
/// firmware is built as `target` (a variant builds as its parent).
pub fn plan(project: &str, path: &Path, board: &str, target: &str) -> Result<Plan, Error> {
    crate::safe_id(board)?;
    let &(_, base, _) = BOARDS
        .iter()
        .find(|(id, _, _)| *id == target)
        .ok_or_else(|| Error::UnknownBoard(target.to_owned()))?;
    let libraries = esp32bench_distro::dirs::user_sketchbook()
        .map(|s| s.join("libraries"))
        .filter(|l| l.is_dir());
    Ok(Plan {
        project: project.to_owned(),
        board: board.to_owned(),
        // DIO works on every board and is what QEMU boots, as for library builds.
        fqbn: format!("{base}:FlashMode=dio"),
        path: path.to_path_buf(),
        libraries,
    })
}

/// Where a sketch build goes: under `root/<project id>/<board>/`.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub build: PathBuf,
    pub out: PathBuf,
}

impl Dirs {
    pub fn new(root: &Path, plan: &Plan) -> Self {
        let base = root.join(&plan.project).join(&plan.board);
        Dirs {
            build: base.join("build"),
            out: base.join("out"),
        }
    }
}

pub fn compile_args(plan: &Plan, dirs: &Dirs) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        "compile".into(),
        "--fqbn".into(),
        plan.fqbn.clone().into(),
        "--build-path".into(),
        dirs.build.clone().into(),
        "--output-dir".into(),
        dirs.out.clone().into(),
    ];
    if let Some(libs) = &plan.libraries {
        a.push("--libraries".into());
        a.push(libs.clone().into());
    }
    // The pinned ESP32 core (3.3.12) copies every build's outputs into the sketch folder
    // (`<sketch>/build/<fqbn>/`, its "savehex" hooks): switched off, so a build writes
    // nothing into the user's folder, and a watched folder does not change with each
    // build. An empty pattern makes arduino-cli skip the hook.
    for n in 0..SAVEHEX_HOOKS {
        for os in ["", ".windows"] {
            a.push("--build-property".into());
            a.push(format!("recipe.hooks.savehex.postsavehex.{n}.pattern{os}=").into());
        }
    }
    a.push(plan.path.clone().into());
    a
}

/// How many "savehex" hooks the pinned ESP32 core has (its platform.txt).
const SAVEHEX_HOOKS: usize = 6;

/// Build the sketch, every line arduino-cli prints going to `log`, then copy its image
/// and ELF to the names the core uses for a project's build.
pub async fn run(
    plan: &Plan,
    tools: &Toolchain,
    dirs: &Dirs,
    mut log: impl FnMut(Stream, String),
) -> Result<Output, Error> {
    tokio::fs::create_dir_all(&dirs.build).await?;
    tokio::fs::create_dir_all(&dirs.out).await?;
    tokio::fs::create_dir_all(&tools.sketchbook).await?;
    step(tools, "compiling", compile_args(plan, dirs), &mut log).await?;
    let name = plan
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = Output {
        image: dirs.out.join(format!("{}.ino.merged.bin", plan.project)),
        elf: dirs.out.join(format!("{}.ino.elf", plan.project)),
    };
    for (from, to) in [
        (dirs.out.join(format!("{name}.ino.merged.bin")), &out.image),
        (dirs.out.join(format!("{name}.ino.elf")), &out.elf),
    ] {
        if !from.is_file() {
            return Err(Error::MissingOutput(from));
        }
        tokio::fs::copy(&from, to).await?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_a_sketch_folder_by_its_main_file() {
        let t = tempfile::tempdir().unwrap();
        let sketch = t.path().join("Blinky");
        std::fs::create_dir_all(sketch.join("src")).unwrap();
        std::fs::write(sketch.join("Blinky.ino"), "void setup(){}\nvoid loop(){}\n").unwrap();
        std::fs::write(sketch.join("pins.h"), "").unwrap();
        std::fs::write(sketch.join("src/util.cpp"), "").unwrap();
        std::fs::write(sketch.join("notes.txt"), "").unwrap();
        let canon = sketch.canonicalize().unwrap();
        assert_eq!(folder(&sketch), Some(canon.clone()));
        assert_eq!(folder(&sketch.join("Blinky.ino")), Some(canon));
        assert_eq!(sources(&sketch), ["Blinky.ino", "pins.h", "src/util.cpp"]);
        // A folder without <name>.ino is not a sketch.
        assert_eq!(folder(t.path()), None);
    }

    #[test]
    fn builds_in_place_for_a_library_board() {
        let p = plan(
            "local-0123456789ab",
            Path::new("/home/u/Blinky"),
            "esp32c3-supermini",
            "esp32c3-supermini",
        )
        .unwrap();
        assert_eq!(
            p.fqbn,
            "esp32:esp32:nologo_esp32c3_super_mini:FlashMode=dio"
        );
        let d = Dirs::new(Path::new("/c/builds"), &p);
        let args = compile_args(&p, &d);
        assert_eq!(args.last().unwrap(), "/home/u/Blinky");
        // The core's copy into the sketch folder is switched off, on every system.
        assert!(args.contains(&OsString::from(
            "recipe.hooks.savehex.postsavehex.5.pattern="
        )));
        assert!(args.contains(&OsString::from(
            "recipe.hooks.savehex.postsavehex.0.pattern.windows="
        )));
        assert!(args.contains(&OsString::from("--build-path")));
        assert!(matches!(
            plan("local-x", Path::new("/s"), "esp32", "nope"),
            Err(Error::UnknownBoard(_))
        ));
    }
}
