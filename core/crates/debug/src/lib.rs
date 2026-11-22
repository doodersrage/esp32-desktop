//! A GDB/MI client: parse MI output records, and drive a toolchain GDB attached to a
//! target (QEMU's stub now; OpenOCD for real boards later).
//!
//! MI values become JSON: tuples are objects, lists are arrays (a list of `name=value`
//! results keeps only the values, so `stack=[frame={..},frame={..}]` is an array of
//! frame objects).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not start GDB: {0}")]
    Spawn(std::io::Error),
    #[error("GDB: {0}")]
    Gdb(String),
    #[error("GDB did not answer `{0}` in time")]
    Timeout(String),
    #[error("GDB has exited")]
    Exited,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// One line of MI output.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    /// `^class,results`, answering the command with `token`.
    Result {
        token: Option<u64>,
        class: String,
        results: Value,
    },
    /// `*class,results`: execution state (stopped, running).
    Exec { class: String, results: Value },
    /// `=class,results`: other notifications (breakpoint-modified, thread-created...).
    Notify { class: String, results: Value },
    /// `~"text"` (console), `@"text"` (target), `&"text"` (log).
    Stream { kind: char, text: String },
    /// `(gdb)`.
    Prompt,
}

/// Parse one MI output line. Returns None for lines that are not MI.
pub fn parse_record(line: &str) -> Option<Record> {
    let line = line.trim_end();
    if line == "(gdb)" {
        return Some(Record::Prompt);
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let token = line[..digits].parse().ok();
    let rest = &line[digits..];
    let kind = rest.chars().next()?;
    let body = &rest[1..];
    match kind {
        '~' | '@' | '&' => {
            let mut p = Parser {
                s: body.as_bytes(),
                i: 0,
            };
            Some(Record::Stream {
                kind,
                text: p.cstring()?,
            })
        }
        '^' | '*' | '=' => {
            let (class, results) = match body.split_once(',') {
                Some((c, r)) => (c.to_owned(), parse_results(r)?),
                None => (body.to_owned(), Value::Object(Map::new())),
            };
            Some(match kind {
                '^' => Record::Result {
                    token,
                    class,
                    results,
                },
                '*' => Record::Exec { class, results },
                _ => Record::Notify { class, results },
            })
        }
        _ => None,
    }
}

/// Parse `a=1,b={c="2"}` into an object.
pub fn parse_results(s: &str) -> Option<Value> {
    let mut p = Parser {
        s: s.as_bytes(),
        i: 0,
    };
    let v = p.results(None)?;
    (p.i == p.s.len()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// `name=value,...` up to `end` (or the end of input) into an object.
    fn results(&mut self, end: Option<u8>) -> Option<Value> {
        let mut map = Map::new();
        loop {
            if self.peek() == end {
                return Some(Value::Object(map));
            }
            let (k, v) = self.result()?;
            map.insert(k, v);
            if !self.eat(b',') {
                return (self.peek() == end).then_some(Value::Object(map));
            }
        }
    }

    fn result(&mut self) -> Option<(String, Value)> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == b'=' {
                break;
            }
            self.i += 1;
        }
        let name = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        if !self.eat(b'=') || name.is_empty() {
            return None;
        }
        Some((name, self.value()?))
    }

    fn value(&mut self) -> Option<Value> {
        match self.peek()? {
            b'"' => self.cstring().map(Value::String),
            b'{' => {
                self.i += 1;
                let v = self.results(Some(b'}'))?;
                self.eat(b'}').then_some(v)
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                while self.peek()? != b']' {
                    // A list holds either values or `name=value` results; keep values.
                    let item = match self.peek()? {
                        b'"' | b'{' | b'[' => self.value()?,
                        _ => self.result()?.1,
                    };
                    items.push(item);
                    if !self.eat(b',') {
                        break;
                    }
                }
                self.eat(b']').then_some(Value::Array(items))
            }
            _ => None,
        }
    }

    /// A C string with backslash escapes.
    fn cstring(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                b'"' => return Some(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'e' => 0x1b,
                        // Octal escapes (\303\251): GDB uses them for non-ASCII bytes.
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            u8::try_from(v).unwrap_or(b'?')
                        }
                        other => other,
                    });
                }
                other => out.push(other),
            }
        }
    }
}

/// An execution event from GDB.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Running,
    /// `*stopped` with its fields (reason, frame, ...).
    Stopped(Value),
    /// GDB's own output on its console (`~` records), for a log.
    Console(String),
    Exited,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A running GDB in MI mode. Dropping it kills GDB.
pub struct Gdb {
    _child: Child,
    stdin: ChildStdin,
    pending: Pending,
    next_token: u64,
}

impl Gdb {
    /// Start `gdb` on `elf` in MI mode, with no init files. Execution events go to the
    /// returned receiver.
    pub async fn start(
        gdb: &Path,
        elf: &Path,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Event>), Error> {
        // The ELF comes from a downloaded project: before GDB reads it, turn off
        // auto-loading (an ELF can carry GDB scripts in .debug_gdb_scripts) and
        // debuginfod (no network).
        let args: [OsString; 8] = [
            "--interpreter=mi3".into(),
            "--nx".into(),
            "-q".into(),
            "-iex".into(),
            "set auto-load off".into(),
            "-iex".into(),
            "set debuginfod enabled off".into(),
            elf.into(),
        ];
        let mut child = Command::new(gdb)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::Spawn)?;
        let stdin = child.stdin.take().ok_or(Error::Exited)?;
        let stdout = child.stdout.take().ok_or(Error::Exited)?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match parse_record(&line) {
                    Some(Record::Result {
                        token: Some(t),
                        class,
                        results,
                    }) => {
                        if let Some(reply) = reader_pending.lock().await.remove(&t) {
                            let r = if class == "error" {
                                Err(results["msg"].as_str().unwrap_or("error").to_owned())
                            } else {
                                Ok(results)
                            };
                            let _ = reply.send(r);
                        }
                    }
                    Some(Record::Exec { class, results }) => {
                        let ev = match class.as_str() {
                            "running" => Event::Running,
                            "stopped" => Event::Stopped(results),
                            _ => continue,
                        };
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Some(Record::Stream { kind: '~', text }) => {
                        let _ = tx.send(Event::Console(text));
                    }
                    _ => {}
                }
            }
            // GDB is gone: fail whatever was waiting.
            for (_, reply) in reader_pending.lock().await.drain() {
                let _ = reply.send(Err("GDB has exited".into()));
            }
            let _ = tx.send(Event::Exited);
        });
        let mut gdb = Gdb {
            _child: child,
            stdin,
            pending,
            next_token: 1,
        };
        gdb.command("-gdb-set confirm off").await?;
        gdb.command("-gdb-set pagination off").await?;
        // Accept commands (interrupt above all) while the target runs.
        gdb.command("-gdb-set mi-async on").await?;
        Ok((gdb, rx))
    }

    /// Send an MI command and wait for its result record.
    pub async fn command(&mut self, cmd: &str) -> Result<Value, Error> {
        if cmd.contains('\n') {
            return Err(Error::Gdb("commands are one line".into()));
        }
        let token = self.next_token;
        self.next_token += 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(token, tx);
        self.stdin
            .write_all(format!("{token}{cmd}\n").as_bytes())
            .await?;
        match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
            Err(_) => Err(Error::Timeout(cmd.to_owned())),
            Ok(Err(_)) => Err(Error::Exited),
            Ok(Ok(r)) => r.map_err(Error::Gdb),
        }
    }

    /// Connect to a GDB remote stub on this machine.
    pub async fn connect(&mut self, port: u16) -> Result<(), Error> {
        self.command(&format!("-target-select remote 127.0.0.1:{port}"))
            .await
            .map(|_| ())
    }

    /// Set a hardware breakpoint (flash code needs one) at a function or `file:line`,
    /// stopping only when `condition` (a C expression) is true, if one is given, and
    /// not the first `ignore` times it is reached.
    pub async fn break_insert(
        &mut self,
        location: &str,
        condition: Option<&str>,
        ignore: u32,
    ) -> Result<Value, Error> {
        if !valid_location(location) {
            return Err(Error::Gdb(format!(
                "`{location}` is not a function or file:line"
            )));
        }
        let mut cmd = String::from("-break-insert -h");
        if ignore > 0 {
            cmd += &format!(" -i {ignore}");
        }
        match condition {
            Some(c) if !valid_expression(c) => {
                return Err(Error::Gdb(
                    "a condition is one line, without GDB's $_ functions".into(),
                ));
            }
            Some(c) => cmd += &format!(" -c {}", mi_quote(c)),
            None => {}
        }
        cmd += &format!(" {location}");
        let r = self.command(&cmd).await?;
        Ok(r["bkpt"].clone())
    }

    /// Pass breakpoint `number` the next `count` times it is reached (0: stop again).
    pub async fn ignore(&mut self, number: u32, count: u32) -> Result<(), Error> {
        self.command(&format!("-break-after {number} {count}"))
            .await
            .map(|_| ())
    }

    /// How many times breakpoint `number` has been reached, ignored passes included.
    pub async fn hits(&mut self, number: u32) -> Result<u32, Error> {
        let r = self.command(&format!("-break-info {number}")).await?;
        Ok(r["BreakpointTable"]["body"][0]["times"]
            .as_str()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0))
    }

    pub async fn break_delete(&mut self, number: u32) -> Result<(), Error> {
        self.command(&format!("-break-delete {number}"))
            .await
            .map(|_| ())
    }

    /// Resume or step: "continue", "next", "step", "finish", or "interrupt" to pause.
    pub async fn control(&mut self, action: &str) -> Result<(), Error> {
        let cmd = match action {
            "continue" => "-exec-continue",
            "next" => "-exec-next",
            "step" => "-exec-step",
            "finish" => "-exec-finish",
            "interrupt" => "-exec-interrupt",
            other => return Err(Error::Gdb(format!("unknown action `{other}`"))),
        };
        self.command(cmd).await.map(|_| ())
    }

    /// Run until `location` (a function or `file:line`), through a temporary hardware
    /// breakpoint that GDB deletes when it is hit, or at a breakpoint before it.
    pub async fn run_to(&mut self, location: &str) -> Result<(), Error> {
        if !valid_location(location) {
            return Err(Error::Gdb(format!(
                "`{location}` is not a function or file:line"
            )));
        }
        self.command(&format!("-break-insert -t -h {location}"))
            .await?;
        self.command("-exec-continue").await.map(|_| ())
    }

    /// The value of a C or C++ expression in the frame the firmware stopped in, as GDB
    /// prints it (`count * 2`, `buf[3]`, `*sensor`).
    pub async fn evaluate(&mut self, expression: &str) -> Result<String, Error> {
        if !valid_expression(expression) {
            return Err(Error::Gdb(
                "an expression is one line, without GDB's $_ functions".into(),
            ));
        }
        let r = self
            .command(&format!(
                "-data-evaluate-expression {}",
                mi_quote(expression)
            ))
            .await?;
        Ok(r["value"].as_str().unwrap_or("").to_owned())
    }

    /// The innermost frame's arguments and local variables, at most 64: name, type,
    /// whether it is an argument, and the value of those GDB prints simply (numbers,
    /// pointers, enums; not structs or arrays). Empty where there is no debug
    /// information (GDB says "No symbol table info available").
    pub async fn locals(&mut self) -> Result<Vec<Local>, Error> {
        let r = match self.command("-stack-list-variables --simple-values").await {
            Ok(r) => r,
            Err(Error::Gdb(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(r["variables"]
            .as_array()
            .into_iter()
            .flatten()
            .take(64)
            .filter_map(|v| {
                Some(Local {
                    name: v["name"].as_str()?.to_owned(),
                    kind: v["type"].as_str().unwrap_or("").to_owned(),
                    arg: v["arg"].as_str() == Some("1"),
                    value: v["value"].as_str().map(str::to_owned),
                })
            })
            .collect())
    }

    /// Make frame `level` (0 is where the firmware stopped) the one locals, registers
    /// and expressions are read in, until the firmware runs again.
    pub async fn select_frame(&mut self, level: u32) -> Result<(), Error> {
        self.command(&format!("-stack-select-frame {level}"))
            .await
            .map(|_| ())
    }

    /// Stop when `expression` (a variable, `buf[3]`, `*ptr`) changes: a watchpoint.
    /// The stop says its old and new value. Returns GDB's `wpt` (its number, and the
    /// expression as GDB read it); it is deleted like a breakpoint.
    pub async fn watch(&mut self, expression: &str) -> Result<Value, Error> {
        if !valid_expression(expression) {
            return Err(Error::Gdb(
                "an expression is one line, without GDB's $_ functions".into(),
            ));
        }
        let r = self
            .command(&format!("-break-watch {}", mi_quote(expression)))
            .await?;
        Ok(r["wpt"].clone())
    }

    /// `length` bytes of memory from the address `at` evaluates to (an address, a
    /// pointer, `&config`): its start address and the bytes. At most
    /// [`MAX_MEMORY`]; fewer where the memory cannot be read.
    pub async fn memory(&mut self, at: &str, length: u32) -> Result<(u64, Vec<u8>), Error> {
        if !valid_expression(at) {
            return Err(Error::Gdb(
                "an address is one line, without GDB's $_ functions".into(),
            ));
        }
        let length = length.clamp(1, MAX_MEMORY);
        let r = self
            .command(&format!(
                "-data-read-memory-bytes {} {length}",
                mi_quote(at)
            ))
            .await?;
        let first = &r["memory"][0];
        let begin = first["begin"]
            .as_str()
            .and_then(|b| u64::from_str_radix(b.trim_start_matches("0x"), 16).ok())
            .ok_or_else(|| Error::Gdb("no memory there".into()))?;
        let hex = first["contents"].as_str().unwrap_or("");
        let bytes = (0..hex.len() / 2)
            .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect();
        Ok((begin, bytes))
    }

    /// The members of a structure or the elements of an array (at most
    /// [`MAX_CHILDREN`]), through a GDB variable object made for the occasion: each
    /// with an expression that names it, to look inside it in turn.
    pub async fn children(&mut self, expression: &str) -> Result<Vec<Member>, Error> {
        if !valid_expression(expression) {
            return Err(Error::Gdb(
                "an expression is one line, without GDB's $_ functions".into(),
            ));
        }
        let var = self
            .command(&format!("-var-create - * {}", mi_quote(expression)))
            .await?;
        let name = var["name"].as_str().unwrap_or("").to_owned();
        let listed = self
            .command(&format!(
                "-var-list-children --simple-values {name} 0 {MAX_CHILDREN}"
            ))
            .await;
        let mut out = Vec::new();
        if let Ok(list) = &listed {
            for c in list["children"].as_array().into_iter().flatten() {
                let child = c["name"].as_str().unwrap_or("");
                // Access specifiers (public, private) are levels of their own in C++.
                let exp = c["exp"].as_str().unwrap_or("").to_owned();
                let path = self
                    .command(&format!("-var-info-path-expression {child}"))
                    .await
                    .ok()
                    .and_then(|p| p["path_expr"].as_str().map(str::to_owned));
                out.push(Member {
                    name: exp,
                    expression: path,
                    kind: c["type"].as_str().unwrap_or("").to_owned(),
                    value: c["value"].as_str().map(str::to_owned),
                    children: c["numchild"]
                        .as_str()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                });
            }
        }
        let _ = self.command(&format!("-var-delete {name}")).await;
        listed.map(|_| out)
    }

    /// Set `expression` (a variable, `buf[3]`) to `value` in the paused frame, as
    /// `expression = value` in C; the value it has then.
    pub async fn assign(&mut self, expression: &str, value: &str) -> Result<String, Error> {
        if !valid_expression(expression) || !valid_expression(value) || value.contains(';') {
            return Err(Error::Gdb(
                "an expression and a value, each one line, without GDB's $_ functions".into(),
            ));
        }
        self.evaluate(&format!("({expression}) = ({value})")).await
    }

    pub async fn frames(&mut self) -> Result<Value, Error> {
        Ok(self.command("-stack-list-frames 0 31").await?["stack"].clone())
    }

    /// General registers as name/value pairs (registers with empty names are skipped).
    /// The registers a person reads while debugging: the program counter and the
    /// general registers (a0-a15 and a few specials on Xtensa, x0-x31 by ABI name on
    /// RISC-V), in GDB's order. Not every register GDB knows: QEMU cannot read some
    /// CSRs (the C3's htimedelta), and asking for those fails the whole request.
    pub async fn registers(&mut self) -> Result<Vec<(String, String)>, Error> {
        let names = self.command("-data-list-register-names").await?;
        let names: Vec<String> = names["register-names"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| n.as_str().unwrap_or("").to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let wanted: Vec<usize> = (0..names.len())
            .filter(|&i| is_general_register(&names[i]))
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let list: Vec<String> = wanted.iter().map(usize::to_string).collect();
        let values = self
            .command(&format!("-data-list-register-values x {}", list.join(" ")))
            .await?;
        let mut out = Vec::new();
        for v in values["register-values"].as_array().into_iter().flatten() {
            let n: usize = v["number"]
                .as_str()
                .and_then(|n| n.parse().ok())
                .unwrap_or(usize::MAX);
            if let (Some(name), Some(value)) = (names.get(n), v["value"].as_str()) {
                out.push((name.clone(), value.to_owned()));
            }
        }
        Ok(out)
    }
}

/// The most members or elements [`Gdb::children`] lists.
pub const MAX_CHILDREN: u32 = 100;

/// A member of a structure, or an element of an array.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Member {
    /// As GDB names it: the member's name, or the element's index.
    pub name: String,
    /// An expression for it, to look inside it in turn (`config.wifi.ssid`, `buf[3]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    /// Absent for values GDB does not print simply (structs, arrays).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// How many members or elements it has in turn.
    #[serde(skip_serializing_if = "is_zero")]
    pub children: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// An argument or local variable of the frame the firmware stopped in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Local {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub arg: bool,
    /// Absent for values GDB does not print simply (structs, arrays).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Registers shown by [`Gdb::registers`], by their GDB names on Xtensa and RISC-V.
pub fn is_general_register(name: &str) -> bool {
    const SPECIAL: &[&str] = &[
        "pc",
        "ps",
        "sar",
        "lbeg",
        "lend",
        "lcount",
        "windowbase",
        "windowstart",
        // RISC-V ABI names for x0-x31.
        "zero",
        "ra",
        "sp",
        "gp",
        "tp",
        "fp",
    ];
    if SPECIAL.contains(&name) {
        return true;
    }
    // a0-a15 (Xtensa's window, RISC-V's a0-a7), t0-t6, s0-s11.
    let (prefix, digits) = name.split_at(name.len().min(1));
    match (prefix, digits.parse::<u8>()) {
        ("a", Ok(n)) => n <= 15 && digits == n.to_string(),
        ("t", Ok(n)) => n <= 6 && digits == n.to_string(),
        ("s", Ok(n)) => n <= 11 && digits == n.to_string(),
        _ => false,
    }
}

/// The GDB release that the pinned Arduino ESP32 core (3.3.12) installs.
pub const ARDUINO_GDB_VERSION: &str = "17.1_20260402";

/// The toolchain GDB for `chip` ("esp32", "esp32s3", "esp32c3") in an Arduino data
/// directory (`esp32bench_distro::dirs::arduino_data`), if the pinned core installed it.
pub fn find_gdb(arduino_data: &Path, chip: &str) -> Option<std::path::PathBuf> {
    let (tool, exe) = match chip {
        "esp32" => ("xtensa-esp-elf-gdb", "xtensa-esp32-elf-gdb"),
        "esp32s3" => ("xtensa-esp-elf-gdb", "xtensa-esp32s3-elf-gdb"),
        "esp32c3" => ("riscv32-esp-elf-gdb", "riscv32-esp-elf-gdb"),
        _ => return None,
    };
    let path = arduino_data
        .join("packages/esp32/tools")
        .join(tool)
        .join(ARDUINO_GDB_VERSION)
        .join("bin")
        .join(format!("{exe}{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

/// A breakpoint location is a function name or `file:line`; nothing GDB could read as
/// another command or option.
pub fn valid_location(loc: &str) -> bool {
    !loc.is_empty()
        && loc.len() <= 256
        && !loc.starts_with('-')
        && loc
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '/' | '-'))
}

/// The most bytes [`Gdb::memory`] reads at once.
pub const MAX_MEMORY: u32 = 1024;

/// A string as an MI c-string argument.
fn mi_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A kept breakpoint (breakpoints.get/set) is its location, maybe followed by ` if `
/// and a condition, as GDB's own `break` writes it: `blink.ino:12 if i == 10`. Its
/// location and condition, if both are valid.
pub fn split_breakpoint(kept: &str) -> Option<(&str, Option<&str>)> {
    let (location, condition) = match kept.split_once(" if ") {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (kept.trim(), None),
    };
    (valid_location(location) && condition.is_none_or(valid_expression))
        .then_some((location, condition))
}

/// An expression for [`Gdb::evaluate`]: one line of at most 256 characters, and none
/// of GDB's convenience functions (`$_shell` runs a shell command); registers (`$pc`)
/// and the program's own names are fine.
pub fn valid_expression(expr: &str) -> bool {
    !expr.trim().is_empty()
        && expr.len() <= 256
        && !expr.chars().any(char::is_control)
        && !expr.contains("$_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_records_seen_from_gdb_17() {
        let r = parse_record(
            r#"*stopped,reason="breakpoint-hit",disp="keep",bkptno="1",frame={addr="0x400d7410",func="app_main",args=[],file="hello.c",line="21"},thread-id="1""#,
        )
        .unwrap();
        let Record::Exec { class, results } = r else {
            panic!()
        };
        assert_eq!(class, "stopped");
        assert_eq!(results["reason"], "breakpoint-hit");
        assert_eq!(results["frame"]["func"], "app_main");
        assert_eq!(results["frame"]["line"], "21");
        assert_eq!(results["frame"]["args"], json!([]));

        let r = parse_record(
            r#"12^done,stack=[frame={level="0",func="a"},frame={level="1",func="b"}]"#,
        )
        .unwrap();
        let Record::Result {
            token,
            class,
            results,
        } = r
        else {
            panic!()
        };
        assert_eq!(token, Some(12));
        assert_eq!(class, "done");
        assert_eq!(results["stack"][1]["func"], "b");

        let r = parse_record(r#"^done,register-names=["pc","ar0",""]"#).unwrap();
        let Record::Result { results, .. } = r else {
            panic!()
        };
        assert_eq!(results["register-names"], json!(["pc", "ar0", ""]));

        assert_eq!(
            parse_record("^running"),
            Some(Record::Result {
                token: None,
                class: "running".into(),
                results: json!({})
            })
        );
        assert_eq!(parse_record("(gdb)"), Some(Record::Prompt));
        assert_eq!(parse_record("random noise"), None);
    }

    #[test]
    fn decodes_stream_escapes() {
        let r = parse_record(r#"~"Hello \"world\"\n\303\251""#).unwrap();
        assert_eq!(
            r,
            Record::Stream {
                kind: '~',
                text: "Hello \"world\"\né".into()
            }
        );
        let r = parse_record(r#"5^error,msg="No symbol \"nope\" in current context.""#).unwrap();
        let Record::Result { class, results, .. } = r else {
            panic!()
        };
        assert_eq!(class, "error");
        assert_eq!(results["msg"], "No symbol \"nope\" in current context.");
    }

    #[test]
    fn picks_general_registers() {
        for r in [
            "pc", "a0", "a15", "sar", "ps", "zero", "ra", "sp", "t6", "s11", "fp",
        ] {
            assert!(is_general_register(r), "{r}");
        }
        for r in [
            "ar0",
            "a16",
            "a01",
            "htimedelta",
            "mstatus",
            "f0",
            "",
            "s",
            "t7",
        ] {
            assert!(!is_general_register(r), "{r}");
        }
    }

    #[test]
    fn kept_breakpoints_may_have_conditions() {
        assert_eq!(split_breakpoint("setup"), Some(("setup", None)));
        assert_eq!(
            split_breakpoint("blink.ino:12 if i == 10"),
            Some(("blink.ino:12", Some("i == 10")))
        );
        assert_eq!(split_breakpoint("blink.ino:12 if "), None);
        assert_eq!(split_breakpoint("x y if i"), None);
        assert_eq!(split_breakpoint("loop if $_shell(\"x\")"), None);
        assert_eq!(mi_quote("a \"b\" \\"), "\"a \\\"b\\\" \\\\\"");
    }

    #[test]
    fn expressions_stay_expressions() {
        assert!(valid_expression("count * 2"));
        assert!(valid_expression("buf[3]"));
        assert!(valid_expression("$pc"));
        assert!(valid_expression("name == \"x\""));
        assert!(!valid_expression(""));
        assert!(!valid_expression("  "));
        assert!(!valid_expression("1\n-gdb-exit"));
        assert!(!valid_expression("$_shell(\"rm x\")"));
        assert!(!valid_expression(&"x".repeat(257)));
    }

    #[test]
    fn breakpoint_locations_cannot_smuggle_commands() {
        assert!(valid_location("setup"));
        assert!(valid_location("hello.c:23"));
        assert!(valid_location("sketch/blink.ino:12"));
        for bad in ["", "-x", "setup\n-gdb-exit", "a b", "x;y", "$(rm)", "a\"b"] {
            assert!(!valid_location(bad), "{bad:?} accepted");
        }
    }
}
