//! Opening a file at a line in the user's own editor (phase 7, the edit loop): a
//! command template such as `kate -l {line} -c {col} {file}`, split into arguments
//! here and run without a shell, or with none the system's default opener.
//!
//! The editor is the user's program, not one of ours: it is started detached and left
//! running when the core exits, unlike the child processes the core supervises.

use std::path::Path;
use std::process::{Command, Stdio};

/// Editors with a known command, by id: what Preferences offers before a custom one.
pub const PRESETS: &[(&str, &str)] = &[
    ("kate", "kate -l {line} -c {col} {file}"),
    ("code", "code -g {file}:{line}:{col}"),
];

/// A preset's template, or `template` itself when it is not a preset's id.
pub fn template_for(name_or_template: &str) -> &str {
    PRESETS
        .iter()
        .find(|(id, _)| *id == name_or_template)
        .map_or(name_or_template, |(_, t)| t)
}

#[derive(Debug)]
pub enum Error {
    BadTemplate(String),
    NotAFile(String),
    Start(String, std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadTemplate(why) => write!(f, "the editor command {why}"),
            Error::NotAFile(path) => write!(f, "{path} is not a file"),
            Error::Start(program, e) => write!(f, "could not start {program}: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The arguments `template` names for `file` at `line` and `column` (1 when unknown):
/// split like a shell would split words (spaces, single and double quotes), with
/// `{file}`, `{line}` and `{col}` filled in inside each argument. No shell runs it, so
/// nothing in the file's name can become a command.
pub fn arguments(
    template: &str,
    file: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<String>, Error> {
    let words = split(template)?;
    if words.is_empty() {
        return Err(Error::BadTemplate("is empty".into()));
    }
    if !words.iter().any(|w| w.contains("{file}")) {
        return Err(Error::BadTemplate("has no {file}".into()));
    }
    if words[0].contains('{') {
        return Err(Error::BadTemplate(
            "must start with the editor's program".into(),
        ));
    }
    let file = file.to_string_lossy();
    Ok(words
        .into_iter()
        .map(|w| {
            w.replace("{file}", &file)
                .replace("{line}", &line.max(1).to_string())
                .replace("{col}", &column.max(1).to_string())
        })
        .collect())
}

fn split(template: &str) -> Result<Vec<String>, Error> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in template.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(Error::BadTemplate("has an unclosed quote".into()));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// Open `file` at `line` and `column`: in the editor `template` names (a preset's id
/// or a template), or with none in the system's default program for it.
pub fn open(file: &Path, line: u32, column: u32, template: Option<&str>) -> Result<(), Error> {
    if !file.is_file() {
        return Err(Error::NotAFile(file.display().to_string()));
    }
    let args = match template.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => arguments(template_for(t), file, line, column)?,
        None => default_opener(file),
    };
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| Error::Start(args[0].clone(), e))?;
    // Reaped when it exits, so it leaves no zombie; it may outlive us.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn default_opener(file: &Path) -> Vec<String> {
    let file = file.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") {
        vec!["open".into(), file]
    } else if cfg!(windows) {
        // explorer opens a file with its associated program, with no shell parsing.
        vec!["explorer".into(), file]
    } else {
        vec!["xdg-open".into(), file]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_the_presets() {
        let f = Path::new("/home/u/my sketch/main.cpp");
        assert_eq!(
            arguments(template_for("kate"), f, 12, 5).unwrap(),
            ["kate", "-l", "12", "-c", "5", "/home/u/my sketch/main.cpp"]
        );
        assert_eq!(
            arguments(template_for("code"), f, 12, 0).unwrap(),
            ["code", "-g", "/home/u/my sketch/main.cpp:12:1"]
        );
    }

    #[test]
    fn splits_a_custom_template_like_a_shell_without_running_one() {
        let f = Path::new("/tmp/a.ino; rm -rf ~");
        let args = arguments(r#"'/opt/My Editor/bin/ed' --goto "{file}:{line}""#, f, 3, 1).unwrap();
        assert_eq!(
            args,
            ["/opt/My Editor/bin/ed", "--goto", "/tmp/a.ino; rm -rf ~:3"]
        );
    }

    #[test]
    fn refuses_bad_templates() {
        let f = Path::new("/tmp/a.c");
        assert!(arguments("", f, 1, 1).is_err());
        assert!(arguments("kate -l {line}", f, 1, 1).is_err());
        assert!(arguments("{file}", f, 1, 1).is_err());
        assert!(arguments("kate '{file}", f, 1, 1).is_err());
    }

    #[test]
    fn refuses_what_is_not_a_file() {
        let d = std::env::temp_dir();
        assert!(matches!(
            open(&d, 1, 1, Some("kate")),
            Err(Error::NotAFile(_))
        ));
    }
}
