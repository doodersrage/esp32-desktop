//! clangd for the user's own projects (phase 7.3): a `compile_commands.json` it can
//! use after every build, and a `.clangd` pointing at it, offered, never forced.
//!
//! ESP-IDF builds write a compilation database naming the project's own files, so its
//! build folder serves as is. arduino-cli compiles a sketch from its working copy
//! (`build/sketch/<name>.ino.cpp`, with prototypes added), so a sketch's database is
//! mapped back to the user's files and written next to the build ([`map_sketch`]).
//!
//! Checked with clangd 23 and `--query-driver` set to the cross-compilers: the
//! sketches of the ESP32 (Xtensa) and the C3 (RISC-V) check with no errors. clangd
//! ignores the GCC-only options below and warns about them; `.clangd` removes them.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// GCC options clangd does not know, which it ignores with a warning: on the ESP32
/// (and S3) the Xtensa ones, on every chip the last two.
pub const REMOVE: &[&str] = &[
    "-mlongcalls",
    "-mdisable-hardware-atomics",
    "-mfix-esp32-psram-cache-issue",
    "-mfix-esp32-psram-cache-strategy=*",
    "-mtext-section-literals",
    "-fstrict-volatile-bitfields",
    "-fno-tree-switch-conversion",
];

/// The first line of a `.clangd` this program wrote, so it may replace its own and
/// never the user's.
pub const MARK: &str = "# Written by ESP32 Bench";

/// The database's file name.
pub const DATABASE: &str = "compile_commands.json";

/// arduino-cli's database for a sketch build (`build/compile_commands.json`), mapped to
/// the user's files: its working copy's `<name>.ino.cpp` becomes `<sketch>/<name>.ino`
/// (compiled as C++ with Arduino.h included, as arduino-cli does), and its copies of the
/// sketch's other files the files themselves. Entries for libraries and the core are
/// kept: they already name real files.
pub fn map_sketch(db: &Value, build: &Path, sketch: &Path) -> Value {
    let copy = build.join("sketch");
    let main = sketch
        .file_name()
        .map(|n| format!("{}.ino.cpp", n.to_string_lossy()))
        .unwrap_or_default();
    let entries = db.as_array().cloned().unwrap_or_default();
    Value::Array(
        entries
            .into_iter()
            .map(|mut e| {
                let Some(file) = e["file"].as_str().map(PathBuf::from) else {
                    return e;
                };
                let Ok(rel) = file.strip_prefix(&copy) else {
                    return e;
                };
                let is_main = rel == Path::new(&main);
                let real = if is_main {
                    sketch.join(main.trim_end_matches(".cpp"))
                } else {
                    sketch.join(rel)
                };
                let old = file.to_string_lossy().into_owned();
                let new = real.to_string_lossy().into_owned();
                if let Some(args) = e["arguments"].as_array_mut() {
                    for a in args.iter_mut() {
                        if a.as_str() == Some(old.as_str()) {
                            *a = json!(new);
                        }
                    }
                    if is_main {
                        // A .ino is C++ with Arduino.h, which the working copy includes.
                        let at = args
                            .iter()
                            .position(|a| a.as_str() == Some(new.as_str()))
                            .unwrap_or(args.len());
                        for (i, extra) in ["-x", "c++", "-include", "Arduino.h"].iter().enumerate()
                        {
                            args.insert(at + i, json!(extra));
                        }
                    }
                } else if let Some(cmd) = e["command"].as_str() {
                    e["command"] = json!(cmd.replace(&old, &new));
                }
                e["file"] = json!(new);
                e
            })
            .collect(),
    )
}

/// Map a sketch build's database (if arduino-cli wrote one) into `out_dir`; returns the
/// folder clangd should read, or `None` when there is no database.
pub fn write_sketch_database(build: &Path, sketch: &Path, out_dir: &Path) -> Option<PathBuf> {
    let db: Value = serde_json::from_slice(&std::fs::read(build.join(DATABASE)).ok()?).ok()?;
    std::fs::create_dir_all(out_dir).ok()?;
    let mapped = map_sketch(&db, build, sketch);
    std::fs::write(
        out_dir.join(DATABASE),
        serde_json::to_vec_pretty(&mapped).ok()?,
    )
    .ok()?;
    Some(out_dir.to_path_buf())
}

/// The `.clangd` for a project whose database is in `database`.
pub fn dot_clangd(database: &Path) -> String {
    let remove = REMOVE.join(", ");
    format!(
        "{MARK} (esp32bench.dev/desktop): where clangd finds how this project builds.\n\
         # clangd also needs --query-driver set to the cross-compilers: see the app's help.\n\
         CompileFlags:\n  CompilationDatabase: {}\n  Remove: [{remove}]\n",
        yaml_string(&database.to_string_lossy())
    )
}

fn yaml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Debug)]
pub enum Refused {
    /// The project has a `.clangd` of its own, which is not ours to replace.
    Theirs(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Theirs(p) => write!(
                f,
                "{} is already there and was not written by this app; it is left as it is",
                p.display()
            ),
            Refused::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Write `<project>/.clangd` pointing at `database`: a new one, or one this program
/// wrote before; never one of the user's.
pub fn write_dot_clangd(project: &Path, database: &Path) -> Result<PathBuf, Refused> {
    let path = project.join(".clangd");
    if let Ok(old) = std::fs::read_to_string(&path)
        && !old.starts_with(MARK)
    {
        return Err(Refused::Theirs(path));
    }
    std::fs::write(&path, dot_clangd(database)).map_err(Refused::Io)?;
    Ok(path)
}

/// The `--query-driver` glob for compilers under `tools` (the Arduino ESP32 core's
/// `packages/esp32/tools`, or ESP-IDF's tools folder): every program in a `bin`.
pub fn query_driver(tools: &Path) -> String {
    format!("{}/**/bin/*", tools.to_string_lossy().replace('\\', "/"))
}

/// What [`setup`] did.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// The `.clangd` written.
    pub clangd: PathBuf,
    /// The folder holding the compilation database.
    pub compilation_database: PathBuf,
    /// The cross-compilers, for clangd's `--query-driver`.
    pub query_driver: String,
}

/// Set up clangd for the user's project `project`, whose build for a board is in `base`
/// (`<builds>/<project id>/<board>`): `.clangd` pointing at that build's database (an
/// ESP-IDF build's own, a sketch's mapped one), the compilers under `tools` for
/// `--query-driver`. Refused before a build, or over a `.clangd` of the user's.
pub fn setup(
    project: &crate::idf::LocalProject,
    base: &Path,
    tools: &Path,
) -> Result<Setup, String> {
    let database = match project.kind {
        crate::idf::LocalKind::Arduino => base.join("clangd"),
        crate::idf::LocalKind::Idf => base.join("idf-build"),
    };
    if !database.join(DATABASE).is_file() {
        return Err(format!(
            "build {} for this board once first: clangd reads how its last build compiled",
            project.name
        ));
    }
    let clangd = write_dot_clangd(&project.path, &database).map_err(|e| e.to_string())?;
    Ok(Setup {
        clangd,
        compilation_database: database,
        query_driver: query_driver(tools),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_working_copy_back_to_the_sketch() {
        let build = Path::new("/c/b/build");
        let sketch = Path::new("/home/u/EditLoop");
        let db = json!([
            { "directory": "/x", "file": "/c/b/build/sketch/EditLoop.ino.cpp",
              "arguments": ["g++", "-c", "-Os", "/c/b/build/sketch/EditLoop.ino.cpp", "-o", "a.o"] },
            { "directory": "/x", "file": "/c/b/build/sketch/src/util.cpp",
              "arguments": ["g++", "-c", "/c/b/build/sketch/src/util.cpp"] },
            { "directory": "/x", "file": "/cores/esp32/main.cpp", "arguments": ["g++", "-c", "/cores/esp32/main.cpp"] },
        ]);
        let m = map_sketch(&db, build, sketch);
        assert_eq!(m[0]["file"], "/home/u/EditLoop/EditLoop.ino");
        assert_eq!(
            m[0]["arguments"],
            json!([
                "g++",
                "-c",
                "-Os",
                "-x",
                "c++",
                "-include",
                "Arduino.h",
                "/home/u/EditLoop/EditLoop.ino",
                "-o",
                "a.o"
            ])
        );
        assert_eq!(m[1]["file"], "/home/u/EditLoop/src/util.cpp");
        assert_eq!(m[1]["arguments"][2], "/home/u/EditLoop/src/util.cpp");
        assert_eq!(m[2], db[2]);
    }

    #[test]
    fn writes_its_own_dot_clangd_and_never_the_users() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("db");
        let written = write_dot_clangd(t.path(), &db).unwrap();
        let text = std::fs::read_to_string(&written).unwrap();
        assert!(text.starts_with(MARK));
        assert!(text.contains("CompilationDatabase:"));
        assert!(text.contains("-mlongcalls"));
        // Ours again: replaced.
        assert!(write_dot_clangd(t.path(), &t.path().join("other")).is_ok());
        // The user's own: left alone.
        std::fs::write(&written, "CompileFlags:\n  Add: [-DMINE]\n").unwrap();
        assert!(matches!(
            write_dot_clangd(t.path(), &db),
            Err(Refused::Theirs(_))
        ));
        assert_eq!(
            std::fs::read_to_string(&written).unwrap(),
            "CompileFlags:\n  Add: [-DMINE]\n"
        );
    }

    #[test]
    fn a_query_driver_for_the_tools_folder() {
        assert_eq!(
            query_driver(Path::new("/home/u/.arduino15/packages/esp32/tools")),
            "/home/u/.arduino15/packages/esp32/tools/**/bin/*"
        );
    }
}
