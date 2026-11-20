//! `esp32bench`: command-line access to the esp32-desktop core.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use esp32bench_sim::uart::WaitOutcome;
use esp32bench_sim::{Arch, Chip, Session, SessionConfig, gdb, image, install, pin};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(name = "esp32bench", version, about = "esp32bench core command line")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Manage the pinned QEMU build.
    #[command(subcommand)]
    Qemu(QemuCmd),
    /// Inspect flash images.
    #[command(subcommand)]
    Image(ImageCmd),
    /// Run firmware in the simulator.
    #[command(subcommand)]
    Sim(SimCmd),
    /// Find connected boards.
    #[command(subcommand)]
    Device(DeviceCmd),
    /// Talk to a board over its serial port.
    #[command(subcommand)]
    Serial(SerialCmd),
    /// Inspect, flash and erase a real board through its ROM bootloader.
    #[command(subcommand)]
    Board(BoardCmd),
    /// Check and install the tools builds and the simulator need.
    #[command(subcommand)]
    Toolchain(ToolchainCmd),
    /// Virtual parts placed on a project's board for the simulator (the same files the
    /// app edits).
    #[command(subcommand)]
    Circuit(CircuitCmd),
    /// The esp32bench.dev project library.
    #[command(subcommand)]
    Library(LibraryCmd),
    /// Your own ESP-IDF projects: folders opened here build with idf.py, and their ids
    /// work wherever a library project's does (build, the app's simulator, flashing).
    #[command(subcommand)]
    Local(LocalCmd),
    /// Build a library project with arduino-cli (or download and check its prebuilt
    /// firmware), or a local ESP-IDF project (`local open`) with idf.py. For a library
    /// project it shows what will run and stops, unless --yes says you agree: library
    /// projects are downloaded code.
    Build {
        /// Project id, as in `library list` or `local list`.
        project: String,
        /// Board id, as in `library show`.
        #[arg(long)]
        board: String,
        /// Build without asking. Only use this after reading the plan.
        #[arg(long)]
        yes: bool,
    },
    /// Call a method on a running esp32benchd and print the result as JSON.
    Rpc {
        /// Method name, e.g. `core.hello`.
        method: String,
        /// Params as a JSON object.
        #[arg(default_value = "{}")]
        params: String,
        /// Daemon socket. Default: $XDG_RUNTIME_DIR/esp32bench/core.sock.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum LocalCmd {
    /// Open an ESP-IDF project folder and print its id. Nothing is written to it.
    Open { path: PathBuf },
    /// The projects opened so far, and the ESP-IDF install builds use.
    List,
    /// Forget a project. Its folder is left as it is.
    Remove { id: String },
}

#[derive(Subcommand)]
enum LibraryCmd {
    /// Download the project library and cache it for offline use.
    Refresh {
        /// https URL or local file to fetch from. Default: $ESP32BENCH_MANIFEST or esp32bench.dev.
        #[arg(long)]
        source: Option<String>,
    },
    /// What an esp32bench:// link from the site opens: its project and board (as
    /// `link.open`). Nothing is built.
    Link {
        /// e.g. esp32bench://project/blink?board=esp32
        url: String,
    },
    /// Show one project: summary, parts and, for one board, wiring and code.
    Show {
        /// Project id, as in `library list`.
        id: String,
        /// Board to show wiring and code for. Default: the project's first board.
        #[arg(long)]
        board: Option<String>,
        /// Print the project as JSON (same as `library.get`).
        #[arg(long)]
        json: bool,
    },
    /// List the cached projects.
    List {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

/// How to reach a board's bootloader.
#[derive(Args)]
struct BoardConn {
    /// Serial port, e.g. /dev/ttyACM0 (see `device list`).
    port: PathBuf,
    /// Do not reset into the bootloader: the board is already in download mode.
    #[arg(long)]
    no_reset: bool,
    /// Use the ROM loader only, without espflash's faster RAM stub.
    #[arg(long)]
    no_stub: bool,
}

impl BoardConn {
    fn options(&self) -> esp32bench_flash::Options {
        esp32bench_flash::Options {
            port: self.port.clone(),
            reset: if self.no_reset {
                esp32bench_flash::Reset::None
            } else {
                esp32bench_flash::Reset::Auto
            },
            use_stub: !self.no_stub,
        }
    }
}

#[derive(Subcommand)]
enum BoardCmd {
    /// Show the chip, its security eFuses and the partition table.
    Inspect {
        #[command(flatten)]
        conn: BoardConn,
    },
    /// Write a merged flash image from offset 0, leaving flash past it alone.
    Flash {
        #[command(flatten)]
        conn: BoardConn,
        image: PathBuf,
        /// Write every byte of the image, erased tail included (to restore a backup
        /// exactly).
        #[arg(long)]
        whole: bool,
    },
    /// Read flash into a file (by default all of it): a backup to restore with
    /// `board flash --whole`.
    Read {
        #[command(flatten)]
        conn: BoardConn,
        out: PathBuf,
        /// Bytes to read. Default: the whole flash.
        #[arg(long)]
        size: Option<u32>,
    },
    /// Erase the whole flash, saved settings included.
    Erase {
        #[command(flatten)]
        conn: BoardConn,
        /// Required: erasing cannot be undone.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ToolchainCmd {
    /// Show what is installed and what is missing.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Install one component: esp32-core, qemu-xtensa, qemu-riscv32, qemu-lcgamboa, or arduino-cli where the distribution has no package for it.
    Install { component: String },
}

#[derive(Subcommand)]
enum SerialCmd {
    /// Print what the board sends (raw bytes) and send it each line typed, until
    /// Ctrl-C or the board goes away.
    Monitor {
        /// Serial port, e.g. /dev/ttyUSB0 (see `device list`).
        port: PathBuf,
        #[arg(long, default_value_t = 115_200)]
        baud: u32,
        /// Reset the board after opening, to see it boot.
        #[arg(long)]
        reset: bool,
        /// End sent lines with CR LF instead of LF.
        #[arg(long)]
        crlf: bool,
    },
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// List serial ports, with the chips each could be (same data as `device.list`).
    List {
        /// Include ports that are not USB.
        #[arg(long)]
        all: bool,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List serial ports, then print the list again each time one appears or disappears.
    Watch {
        /// Include ports that are not USB.
        #[arg(long)]
        all: bool,
        /// Print one JSON object per line instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum QemuCmd {
    /// Download, verify and unpack the pinned QEMU build.
    Install {
        /// Architecture to install: xtensa, riscv32, or all.
        #[arg(long, default_value = "all")]
        arch: String,
    },
    /// Show the pinned release and where each binary is.
    Status,
}

#[derive(Subcommand)]
enum ImageCmd {
    /// Check that a merged image will boot in QEMU.
    Inspect {
        #[arg(long)]
        chip: Chip,
        image: PathBuf,
    },
}

/// Options shared by every command that starts a simulator session.
#[derive(Args)]
struct SessionArgs {
    #[arg(long)]
    chip: Chip,
    image: PathBuf,
    /// Open a GDB stub on 127.0.0.1 at this port.
    #[arg(long)]
    gdb: Option<u16>,
    /// QEMU binary to run instead of the pinned install (e.g. a build of another fork).
    #[arg(long)]
    qemu: Option<PathBuf>,
    /// QEMU machine instead of the chip's default (e.g. `esp32-picsimlab`).
    #[arg(long)]
    machine: Option<String>,
    /// One extra QEMU argument, passed as-is. Repeat for more: `--qemu-arg=-nic --qemu-arg=user`.
    #[arg(long = "qemu-arg", allow_hyphen_values = true)]
    qemu_args: Vec<String>,
    /// QEMU to run on: lcgamboa (the fork `toolchain install qemu-lcgamboa` builds)
    /// or espressif. Default: lcgamboa when it is built, as the app does.
    #[arg(long, conflicts_with = "qemu")]
    engine: Option<esp32bench_sim::Engine>,
    /// GPIOs to bridge (lcgamboa's fork): their changes print on stderr,
    /// and a stdin line `:set <gpio> <0|1>` drives one. E.g. `--pins 2,4`.
    #[arg(long, value_delimiter = ',')]
    pins: Vec<u8>,
    /// Also show the C3's USB serial port in the console, for builds whose Serial is
    /// there (the C3 SuperMini's). lcgamboa's fork only.
    #[arg(long)]
    usb_console: bool,
    /// No simulated Wi-Fi network. By default sessions on lcgamboa's ESP32, S3 and C3
    /// have one: an open access point, PICSimLabWifi, with an address by DHCP and
    /// nothing outside reachable; the sketch's port 80 is forwarded to a local port.
    #[arg(long)]
    no_network: bool,
    /// Do not join library sketches to the simulated network. By default, when a
    /// sketch's settings block reports its Wi-Fi unset, the console sends it the
    /// network's name and an empty password, and SAVE.
    #[arg(long)]
    no_join_wifi: bool,
    /// Count time in instructions (QEMU's -icount): timing like the chip's and
    /// independent of this computer's load, at some cost in speed.
    #[arg(long)]
    icount: bool,
    /// Set by `sim run --circuit`: the circuit for the host to run.
    #[arg(skip)]
    circuit: Option<String>,
}

#[derive(Subcommand)]
enum CircuitCmd {
    /// The kinds of part there are, with their pins, properties and controls.
    Models,
    /// List the parts on a project's board.
    Show { project: String, board: String },
    /// Add a part.
    Add {
        project: String,
        board: String,
        /// The kind of part: see `circuit models`.
        kind: String,
        /// Part id: lowercase letters, digits and dashes.
        id: String,
        /// The GPIO, for a part with one pin.
        #[arg(long)]
        pin: Option<u8>,
        /// GPIOs by pin name, for a part with several: `--pins red=25,green=26,blue=27`.
        #[arg(long, value_delimiter = ',')]
        pins: Vec<String>,
        /// I2C bus and address, for an I2C part: `--i2c 0:0x3c`.
        #[arg(long)]
        i2c: Option<String>,
        /// A property: `--prop activeLow=true`. Repeat for more.
        #[arg(long = "prop")]
        props: Vec<String>,
        #[arg(long, default_value = "")]
        label: String,
    },
    /// Set where a part's slider starts next run (a sensor's reading), e.g.
    /// `circuit control blink esp32 air temperature 30.5`.
    Control {
        project: String,
        board: String,
        id: String,
        control: String,
        value: f64,
    },
    /// The parts a library project's wiring names that the simulator has models for,
    /// read from the cached library. With --apply, place them (replacing the board's
    /// parts).
    Suggest {
        project: String,
        board: String,
        #[arg(long)]
        apply: bool,
    },
    /// Remove a part.
    Remove {
        project: String,
        board: String,
        id: String,
    },
}

/// `name=value` with the value as JSON (true, 3, 2.5); otherwise a string.
fn key_value(s: &str) -> Result<(String, serde_json::Value)> {
    let (k, v) = s
        .split_once('=')
        .with_context(|| format!("`{s}` is not name=value"))?;
    let value = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_owned()));
    Ok((k.to_owned(), value))
}

fn circuits_root() -> Result<PathBuf> {
    Ok(esp32bench_sim::install::data_dir()?.join("circuits"))
}

async fn circuit_command(cmd: CircuitCmd) -> Result<()> {
    let root = circuits_root()?;
    let ids = |project: &str, board: &str| -> Result<()> {
        for id in [project, board] {
            esp32bench_build::safe_id(id)?;
        }
        Ok(())
    };
    match cmd {
        CircuitCmd::Models => {
            for m in esp32bench_circuit::models() {
                let m = m.model;
                let mut what = Vec::new();
                if !m.pins.is_empty() {
                    what.push(format!("pins {}", m.pins.join(",")));
                }
                if let Some(i2c) = &m.i2c {
                    let a: Vec<String> =
                        i2c.addresses.iter().map(|a| format!("{a:#04x}")).collect();
                    what.push(format!("I2C {}", a.join("/")));
                }
                if !m.props.is_empty() {
                    let p: Vec<&str> = m.props.keys().map(String::as_str).collect();
                    what.push(format!("props {}", p.join(",")));
                }
                if !m.controls.is_empty() {
                    let c: Vec<&str> = m.controls.iter().map(|c| c.name.as_str()).collect();
                    what.push(format!("controls {}", c.join(",")));
                }
                println!("{:<10} {:<16} {}", m.id, m.name, what.join("; "));
            }
        }
        CircuitCmd::Suggest {
            project,
            board,
            apply,
        } => {
            ids(&project, &board)?;
            let cache = esp32bench_library::Cache::new(esp32bench_library::cache_dir()?);
            let Some(m) = cache.load().await? else {
                bail!("no project library cached yet; run `esp32bench library refresh`");
            };
            let Some(p) = m.project(&project) else {
                bail!("no project `{project}` in the library");
            };
            let Some(build) = p.builds.iter().find(|b| b.board == board) else {
                bail!("`{project}` has no build for `{board}`");
            };
            let wiring: Vec<esp32bench_circuit::suggest::Wire> = build
                .wiring
                .iter()
                .map(|w| esp32bench_circuit::suggest::Wire {
                    from: &w.from,
                    to: &w.to,
                })
                .collect();
            let hints: Vec<&str> = p.parts.iter().map(|x| x.name.as_str()).collect();
            let s = esp32bench_circuit::suggest::from_wiring(&wiring, &hints);
            s.circuit.validate()?;
            for part in &s.circuit.parts {
                let at = match (&part.pin, &part.i2c) {
                    (Some(g), _) => format!("GPIO {g}"),
                    (_, Some(i)) => format!("I2C {}:{:#04x}", i.bus, i.address),
                    _ => part
                        .pins
                        .iter()
                        .map(|(n, g)| format!("{n}=GPIO {g}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                };
                println!("{:<16} {:<14} {at:<20} {}", part.id, part.kind, part.label);
            }
            if s.circuit.parts.is_empty() {
                println!("no parts the simulator has models for");
            }
            if !s.unmatched.is_empty() {
                println!("not simulated: {}", s.unmatched.join(", "));
            }
            if apply {
                esp32bench_circuit::save(&root, &project, &board, &s.circuit)?;
                println!(
                    "placed {} parts on {project} / {board}",
                    s.circuit.parts.len()
                );
            }
        }
        CircuitCmd::Show { project, board } => {
            ids(&project, &board)?;
            let c = esp32bench_circuit::load(&root, &project, &board)?;
            if c.parts.is_empty() {
                println!("no parts on {project} / {board}");
            }
            for p in &c.parts {
                let Some(m) = esp32bench_circuit::model(&p.kind) else {
                    continue;
                };
                let pins: Vec<String> = p
                    .connections(&m.model)
                    .iter()
                    .map(|(n, g)| format!("{n}=GPIO {g}"))
                    .collect();
                let mut at = match &p.i2c {
                    Some(i) => format!(" I2C {}:{:#04x}", i.bus, i.address),
                    None => String::new(),
                };
                for (n, v) in &p.controls {
                    at.push_str(&format!(" {n}={v}"));
                }
                println!(
                    "{:<16} {:<10} {}{at} {} {}",
                    p.id,
                    p.kind,
                    pins.join(" "),
                    serde_json::Value::Object(p.resolved_props(&m.model)),
                    p.label
                );
            }
        }
        CircuitCmd::Add {
            project,
            board,
            kind,
            id,
            pin,
            pins,
            i2c,
            props,
            label,
        } => {
            ids(&project, &board)?;
            let mut part = esp32bench_circuit::Part {
                id,
                kind,
                pin,
                pins: Default::default(),
                i2c: None,
                props: Default::default(),
                active_low: None,
                label,
                controls: Default::default(),
                at: None,
            };
            for p in &pins {
                let (k, v) = key_value(p)?;
                let gpio = v
                    .as_u64()
                    .and_then(|g| u8::try_from(g).ok())
                    .with_context(|| format!("`{p}`: not a GPIO"))?;
                part.pins.insert(k, gpio);
            }
            for p in &props {
                let (k, v) = key_value(p)?;
                part.props.insert(k, v);
            }
            if let Some(spec) = &i2c {
                let (bus, addr) = spec
                    .split_once(':')
                    .context("--i2c is bus:address, e.g. 0:0x3c")?;
                part.i2c = Some(esp32bench_circuit::I2cPlace {
                    bus: bus.parse().context("bad I2C bus")?,
                    address: u8::from_str_radix(addr.trim_start_matches("0x"), 16)
                        .context("bad I2C address")?,
                });
            }
            let mut c = esp32bench_circuit::load(&root, &project, &board)?;
            c.parts.push(part);
            esp32bench_circuit::save(&root, &project, &board, &c)?;
        }
        CircuitCmd::Control {
            project,
            board,
            id,
            control,
            value,
        } => {
            ids(&project, &board)?;
            let mut c = esp32bench_circuit::load(&root, &project, &board)?;
            let part = c
                .parts
                .iter_mut()
                .find(|p| p.id == id)
                .with_context(|| format!("no part `{id}` on {project} / {board}"))?;
            part.controls.insert(control, value);
            esp32bench_circuit::save(&root, &project, &board, &c)?;
        }
        CircuitCmd::Remove { project, board, id } => {
            ids(&project, &board)?;
            let mut c = esp32bench_circuit::load(&root, &project, &board)?;
            let before = c.parts.len();
            c.parts.retain(|p| p.id != id);
            if c.parts.len() == before {
                bail!("no part `{id}` on {project} / {board}");
            }
            esp32bench_circuit::save(&root, &project, &board, &c)?;
        }
    }
    Ok(())
}

#[derive(Subcommand)]
enum SimCmd {
    /// Boot an image, stream UART0 to stdout and stdin lines to UART0, until QEMU exits or Ctrl-C.
    Run {
        #[command(flatten)]
        session: SessionArgs,
        /// Stay halted until a debugger continues the target.
        #[arg(long)]
        paused: bool,
        /// Save each new picture here: QEMU's virtual RGB panel as frame-NNNN.ppm, and
        /// display parts (an SSD1306...) as <part>-NNNN.pgm.
        #[arg(long)]
        frames: Option<PathBuf>,
        /// A circuit file (schema/circuit) whose parts to run: their pins are bridged,
        /// part changes print on stderr, and stdin lines `:press <id>`,
        /// `:release <id>` and `:control <id> <name> <value>` work them.
        #[arg(long)]
        circuit: Option<PathBuf>,
        /// Record the bridged pins' changes (`--pins`, `--circuit`) with their
        /// simulated times, and write them here as a VCD file when the run ends; it
        /// opens in PulseView or GTKWave.
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// Boot an image and pass if a marker appears on UART0 before the timeout.
    BootTest {
        #[command(flatten)]
        session: SessionArgs,
        /// Text that must appear on UART0. The ROM prints `rst:0x` on every chip.
        #[arg(long, default_value = "rst:0x")]
        expect: String,
        /// Seconds to wait for the marker.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// After the marker, reset over QMP and require the marker again.
        #[arg(long)]
        reset: bool,
        /// Do not echo UART output.
        #[arg(long, short)]
        quiet: bool,
    },
}

/// The process exit status.
#[derive(Clone, Copy)]
struct ExitCode(i32);

impl ExitCode {
    const SUCCESS: ExitCode = ExitCode(0);
    const FAILURE: ExitCode = ExitCode(1);

    fn from(code: u8) -> Self {
        ExitCode(i32::from(code))
    }
}

#[tokio::main]
async fn main() {
    // Exit quietly when stdout is a closed pipe (e.g. `| head`) instead of panicking.
    // SAFETY: setting a signal disposition to the default before any threads use it.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let code = match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    };
    // Exit now rather than return: Tokio's stdin reader is a blocking thread that the
    // runtime would wait on until stdin closes, so `serial monitor` and `sim run`
    // would hang after Ctrl-C when stdin is still open.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    std::process::exit(code.0);
}

async fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Cmd::Qemu(QemuCmd::Install { arch }) => {
            let arches: Vec<Arch> = if arch == "all" {
                Arch::ALL.to_vec()
            } else {
                vec![arch.parse()?]
            };
            for a in arches {
                eprintln!("installing QEMU {} for {a}...", pin::QEMU_RELEASE);
                let path = install::install_qemu(a).await?;
                println!("{a}: {}", path.display());
            }
        }
        Cmd::Qemu(QemuCmd::Status) => {
            println!("pinned release: {}", pin::QEMU_RELEASE);
            for a in Arch::ALL {
                match install::find_qemu(a) {
                    Ok(p) => println!("{a}: {}", p.display()),
                    Err(e) => println!("{a}: {e}"),
                }
            }
        }
        Cmd::Image(ImageCmd::Inspect { chip, image: path }) => {
            let data =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let info = image::inspect(chip, &data)?;
            println!("chip:       {}", info.chip);
            println!("size:       {} bytes", info.len);
            match info.declared_flash_size {
                Some(d) => println!("declared:   {} MiB", d / (1024 * 1024)),
                None => println!("declared:   none QEMU emulates"),
            }
            println!("padded to:  {} MiB", info.flash_size / (1024 * 1024));
            println!("flash mode: {:?}", info.flash_mode);
            if !info.flash_mode.boots_in_qemu() {
                bail!(
                    "flash mode {:?} does not boot in QEMU; rebuild with DIO or DOUT",
                    info.flash_mode
                );
            }
        }
        Cmd::Device(DeviceCmd::List { all, json }) => {
            print_ports(&esp32bench_device::list(all)?, json, true)?;
        }
        Cmd::Device(DeviceCmd::Watch { all, json }) => {
            let mut monitor = esp32bench_device::Monitor::new()?;
            let mut last = esp32bench_device::list(all)?;
            print_ports(&last, json, !json)?;
            // One Ctrl-C listener for the whole loop: a new one each turn would miss a
            // signal that came while the loop was busy, and the handler tokio installs
            // keeps the default one from ending the program.
            let ctrl_c = tokio::signal::ctrl_c();
            tokio::pin!(ctrl_c);
            loop {
                tokio::select! {
                    r = monitor.changed() => r?,
                    _ = &mut ctrl_c => break,
                }
                let now = esp32bench_device::list(all)?;
                if now != last {
                    if !json {
                        println!("--");
                    }
                    print_ports(&now, json, !json)?;
                    last = now;
                }
            }
        }
        Cmd::Library(LibraryCmd::Refresh { source }) => {
            let source = source.unwrap_or_else(esp32bench_library::default_source);
            let cache = esp32bench_library::Cache::new(esp32bench_library::cache_dir()?);
            let m = cache.refresh(&source).await?;
            println!(
                "{} projects (manifest {}, generated {}) cached in {}",
                m.projects.len(),
                m.version,
                m.generated,
                cache.path().display()
            );
        }
        Cmd::Library(LibraryCmd::Link { url }) => {
            let link = esp32bench_library::link::parse(&url).map_err(anyhow::Error::msg)?;
            let cache = esp32bench_library::Cache::new(esp32bench_library::cache_dir()?);
            let Some(m) = cache.load().await? else {
                bail!("no project library cached yet; run `esp32bench library refresh`");
            };
            let Some(p) = m.project(&link.project) else {
                bail!("no project `{}` in the library", link.project);
            };
            let board = link
                .board
                .filter(|b| p.builds.iter().any(|x| &x.board == b));
            println!("{}  {}", p.id, board.as_deref().unwrap_or("(no board)"));
        }
        Cmd::Library(LibraryCmd::Show { id, board, json }) => {
            let cache = esp32bench_library::Cache::new(esp32bench_library::cache_dir()?);
            let Some(m) = cache.load().await? else {
                bail!("no project library cached yet; run `esp32bench library refresh`");
            };
            let Some(p) = m.project(&id) else {
                bail!("no project `{id}` in the library");
            };
            if json {
                println!("{}", serde_json::to_string_pretty(p)?);
                return Ok(ExitCode::SUCCESS);
            }
            let build = match &board {
                Some(b) => p.builds.iter().find(|x| &x.board == b).with_context(|| {
                    let have: Vec<&str> = p.builds.iter().map(|x| x.board.as_str()).collect();
                    format!("`{id}` has no build for `{b}`; boards: {}", have.join(", "))
                })?,
                None => p.builds.first().context("project has no builds")?,
            };
            println!(
                "{} ({}, {} min)\n{}\n{}\n",
                p.name, p.level, p.minutes, p.summary, p.url
            );
            if !p.parts.is_empty() {
                println!("Parts:");
                for part in &p.parts {
                    println!("  {} x {}", part.quantity, part.name);
                }
            }
            if !p.libraries.is_empty() {
                println!("Libraries: {}", p.libraries.join(", "));
            }
            println!("\nBoard {}:", build.board);
            for w in &build.wiring {
                println!("  {:<10} -> {:<14} {}", w.from, w.to, w.note);
            }
            match &build.code {
                Some(code) => println!("\n{code}"),
                None => println!("\nPrebuilt firmware; no sketch."),
            }
        }
        Cmd::Library(LibraryCmd::List { json }) => {
            let cache = esp32bench_library::Cache::new(esp32bench_library::cache_dir()?);
            let Some(m) = cache.load().await? else {
                bail!("no project library cached yet; run `esp32bench library refresh`");
            };
            let list = m.summaries();
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for p in &list {
                    println!(
                        "{:<28} {:<13} {:>3} min  {:<9} {}",
                        p.id,
                        p.level,
                        p.minutes,
                        if p.firmware == esp32bench_library::Firmware::Sketch {
                            "sketch"
                        } else {
                            "prebuilt"
                        },
                        p.chips.join(",")
                    );
                }
                println!(
                    "{} projects, manifest {} ({})",
                    list.len(),
                    m.version,
                    m.generated
                );
            }
        }
        Cmd::Local(cmd) => {
            let registry = esp32bench_build::idf::Registry::new(
                esp32bench_sim::install::data_dir()?.join("local-projects.json"),
            );
            match cmd {
                LocalCmd::Open { path } => {
                    let p = registry.add(&path)?;
                    println!("{}  {}  {}", p.id, p.name, p.path.display());
                }
                LocalCmd::List => {
                    for p in registry.list() {
                        println!("{}  {}  {}", p.id, p.name, p.path.display());
                    }
                    match esp32bench_build::idf::detect() {
                        Some(i) => eprintln!("ESP-IDF {} at {}", i.version, i.path.display()),
                        None => eprintln!("{}", esp32bench_build::Error::NoIdf),
                    }
                }
                LocalCmd::Remove { id } => {
                    if !registry.remove(&id)? {
                        bail!("no local project `{id}`");
                    }
                }
            }
        }
        Cmd::Build { project, board, .. } if esp32bench_build::idf::is_local(&project) => {
            // The user's own folder: no consent step (rule 4 is for downloaded code).
            use esp32bench_build::idf;
            let registry = idf::Registry::new(
                esp32bench_sim::install::data_dir()?.join("local-projects.json"),
            );
            let lp = registry
                .get(&project)
                .ok_or_else(|| esp32bench_build::Error::NoLocalProject(project.clone()))?;
            let install = idf::detect().ok_or(esp32bench_build::Error::NoIdf)?;
            let plan = idf::plan(&lp, &board, &install)?;
            eprintln!(
                "Build {} ({}) for {} with ESP-IDF {}",
                lp.name,
                lp.path.display(),
                plan.target,
                install.version
            );
            let dirs = idf::Dirs::new(&esp32bench_library::cache_dir()?.join("builds"), &plan);
            let out = idf::run(&plan, &dirs, |stream, line| match stream {
                esp32bench_build::Stream::Stdout => println!("{line}"),
                esp32bench_build::Stream::Stderr => eprintln!("{line}"),
            })
            .await?;
            eprintln!(
                "\nimage {}\nelf   {}",
                out.image.display(),
                out.elf.display()
            );
        }
        Cmd::Build {
            project,
            board,
            yes,
        } => {
            let cache = esp32bench_library::Cache::new(esp32bench_library::cache_dir()?);
            let Some(m) = cache.load().await? else {
                bail!("no project library cached yet; run `esp32bench library refresh`");
            };
            let (plan, code) = esp32bench_build::plan(&m, &project, &board)?;
            // Prebuilt firmware: downloaded and checked instead of built, after --yes.
            if let Some(image) = &plan.image {
                eprintln!(
                    "Download prebuilt firmware {} for {}",
                    plan.project, plan.board
                );
                eprintln!("  from      {}", image.url);
                eprintln!("  sha256    {} ({} bytes)", image.sha256, image.size);
                if !yes {
                    eprintln!("\nThis firmware is downloaded. Run again with --yes to fetch it.");
                    return Ok(ExitCode::from(2));
                }
                let source = esp32bench_library::image_source(
                    &esp32bench_library::default_source(),
                    &image.url,
                )?;
                let dirs = esp32bench_build::Dirs::new(
                    &esp32bench_library::cache_dir()?.join("builds"),
                    &plan,
                );
                let out = esp32bench_build::fetch_image(&plan, &source, &dirs, |_, line| {
                    eprintln!("{line}")
                })
                .await?;
                eprintln!("\nimage {}", out.image.display());
                return Ok(ExitCode::SUCCESS);
            }
            let sketchbook = esp32bench_build::default_sketchbook()
                .context("no data directory: set XDG_DATA_HOME or HOME")?;
            let tools = esp32bench_build::Toolchain::detect(sketchbook)?;
            eprintln!("Build {} for {} ({})", plan.project, plan.board, plan.fqbn);
            eprintln!("  code      sha256 {}", plan.code_sha256);
            if plan.install.is_empty() {
                eprintln!("  libraries none to download");
            } else {
                eprintln!(
                    "  libraries download and install: {}",
                    plan.install.join(", ")
                );
            }
            if !yes {
                eprintln!(
                    "\nThis project was downloaded. Read it first (`esp32bench library show {project} --board {board}`), then run again with --yes to build it."
                );
                return Ok(ExitCode::from(2));
            }
            let dirs = esp32bench_build::Dirs::new(
                &esp32bench_library::cache_dir()?.join("builds"),
                &plan,
            );
            let out =
                esp32bench_build::run(&plan, &code, &tools, &dirs, |stream, line| match stream {
                    esp32bench_build::Stream::Stdout => println!("{line}"),
                    esp32bench_build::Stream::Stderr => eprintln!("{line}"),
                })
                .await?;
            eprintln!(
                "\nimage {}\nelf   {}",
                out.image.display(),
                out.elf.display()
            );
        }
        Cmd::Serial(SerialCmd::Monitor {
            port,
            baud,
            reset,
            crlf,
        }) => {
            let (mut p, mut rx) = esp32bench_serial::Port::open(&port, baud)?;
            eprintln!(
                "connected to {} at {baud} baud; Ctrl-C to quit",
                port.display()
            );
            if reset {
                p.reset()?;
            }
            let ending = if crlf { "\r\n" } else { "\n" };
            let mut out = std::io::stdout();
            let mut stdin = BufReader::new(tokio::io::stdin()).lines();
            let mut stdin_open = true;
            // One Ctrl-C listener for the whole loop: a new one each turn would miss a
            // signal that came while the loop was busy, and the handler tokio installs
            // keeps the default one from ending the program.
            let ctrl_c = tokio::signal::ctrl_c();
            tokio::pin!(ctrl_c);
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(esp32bench_serial::Event::Data(bytes)) => {
                            out.write_all(&bytes)?;
                            out.flush()?;
                        }
                        Some(esp32bench_serial::Event::Closed(reason)) => {
                            eprintln!("\nport closed: {reason}");
                            return Ok(ExitCode::FAILURE);
                        }
                        None => break,
                    },
                    line = stdin.next_line(), if stdin_open => match line? {
                        Some(l) => p.write(format!("{l}{ending}").as_bytes())?,
                        None => stdin_open = false,
                    },
                    _ = &mut ctrl_c => break,
                }
            }
        }
        Cmd::Circuit(cmd) => circuit_command(cmd).await?,
        Cmd::Board(cmd) => {
            // espflash blocks; it has the terminal to itself here.
            tokio::task::spawn_blocking(move || board_command(cmd)).await??;
        }
        Cmd::Toolchain(cmd) => {
            let env = esp32bench_toolchain::Env {
                arduino_cli: None,
                arduino_data: esp32bench_toolchain::Env::default_arduino_data()
                    .context("no home directory")?,
                sketchbook: esp32bench_build::default_sketchbook()
                    .context("no data directory: set XDG_DATA_HOME or HOME")?,
                distro: esp32bench_distro::Distro::detect(),
            };
            match cmd {
                ToolchainCmd::Status { json } => {
                    let comps = esp32bench_toolchain::status(&env).await;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&comps)?);
                    } else {
                        for c in &comps {
                            let state = serde_json::to_value(c.state)?;
                            let mut line = format!(
                                "{:<14} {:<14} {:<10}",
                                c.id.as_str(),
                                state.as_str().unwrap_or("?"),
                                c.needed_for
                            );
                            if let Some(v) = &c.version {
                                line += &format!(" have {v}");
                            }
                            if let Some(w) = &c.wanted
                                && c.version.as_ref() != Some(w)
                            {
                                line += &format!(" want {w}");
                            }
                            if let Some(b) = c.download_bytes {
                                line += &format!(" ({} MB to download)", b / 1_000_000);
                            }
                            println!("{line}");
                            if let Some(h) = &c.hint {
                                println!("{:<14} {h}", "");
                            }
                        }
                    }
                }
                ToolchainCmd::Install { component } => {
                    let id = esp32bench_toolchain::Id::parse(&component)
                        .with_context(|| format!("unknown component `{component}`"))?;
                    // Ctrl-C must drop the install so its build stops too: build steps
                    // run in their own process group, out of the terminal's reach.
                    tokio::select! {
                        r = esp32bench_toolchain::install(&env, id, |l| println!("{l}")) => r?,
                        _ = tokio::signal::ctrl_c() => bail!("interrupted"),
                    }
                }
            }
        }
        Cmd::Rpc {
            method,
            params,
            socket,
        } => {
            let params: serde_json::Value =
                serde_json::from_str(&params).context("params must be JSON")?;
            let socket = match socket {
                Some(s) => s,
                None => esp32bench_rpc::default_socket_path()
                    .context("no runtime directory (XDG_RUNTIME_DIR is not set); pass --socket")?,
            };
            let mut client = esp32bench_rpc::Client::connect(&socket).await?;
            let result = client.call(&method, params).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Cmd::Sim(SimCmd::Run {
            mut session,
            paused,
            frames,
            circuit,
            trace,
        }) => {
            if trace.is_some() && session.pins.is_empty() && circuit.is_none() {
                bail!("--trace records bridged pins: add --pins or --circuit");
            }
            let mut recorder = trace
                .as_ref()
                .map(|_| esp32bench_sim::trace::Trace::default());
            let circuit = match &circuit {
                Some(path) => {
                    let c: esp32bench_circuit::Circuit =
                        serde_json::from_slice(&std::fs::read(path)?)
                            .with_context(|| format!("reading {}", path.display()))?;
                    c.validate()?;
                    session.pins.extend(c.pins());
                    session.pins.sort_unstable();
                    session.pins.dedup();
                    Some(c)
                }
                None => None,
            };
            let has_parts = circuit.is_some();
            session.circuit = circuit.as_ref().map(serde_json::to_string).transpose()?;
            if let Some(dir) = &frames {
                std::fs::create_dir_all(dir)?;
            }
            if paused && session.gdb.is_none() {
                bail!("--paused needs --gdb, or nothing can resume the target");
            }
            let chip = session.chip;
            let join_wifi = !session.no_join_wifi;
            let mut s = start(session, paused).await?;
            if let Some(n) = s.network() {
                eprintln!(
                    "network: simulated Wi-Fi `{}` (open; nothing outside reachable); \
                     the sketch's web page: {}",
                    esp32bench_sim::network::SSID,
                    n.web_url()
                );
            }
            let mut joiner =
                (s.network().is_some() && join_wifi).then(esp32bench_sim::network::Joiner::new);
            if let Some(addr) = s.gdb_addr() {
                eprintln!("GDB stub on {addr}");
                // The same GDB the app's debugger uses.
                let data = esp32bench_toolchain::Env::default_arduino_data().unwrap_or_default();
                match esp32bench_debug::find_gdb(&data, chip.id()) {
                    Some(gdb) => eprintln!(
                        "attach with: {} -iex 'set auto-load off' -ex 'target remote {addr}' <firmware.elf>",
                        gdb.display()
                    ),
                    None => eprintln!(
                        "GDB comes with the ESP32 core (esp32bench toolchain install esp32-core)"
                    ),
                }
            }
            let mut out = std::io::stdout();
            // Lines typed on stdin go to UART0 RX, ending in CR LF like a serial terminal.
            let mut stdin = BufReader::new(tokio::io::stdin()).lines();
            let mut stdin_open = true;
            // Polling the panel is what lets firmware's draws finish (see sim::display).
            let mut display = esp32bench_sim::display::Display::new(&s.paths().dir.clone());
            let mut saved = 0u32;
            let mut gpio = s.gpio.take();
            // One Ctrl-C listener for the whole loop: a new one each turn would miss a
            // signal that came while the loop was busy, and the handler tokio installs
            // keeps the default one from ending the program.
            let ctrl_c = tokio::signal::ctrl_c();
            tokio::pin!(ctrl_c);
            loop {
                tokio::select! {
                    ev = async {
                        match gpio.as_mut() {
                            Some(g) => g.next_event().await,
                            None => std::future::pending().await,
                        }
                    } => match ev?.inspect(|ev| {
                        if let Some(r) = recorder.as_mut() {
                            r.record(ev);
                        }
                    }) {
                        // An input the host drove: only for the trace.
                        Some(esp32bench_sim::gpio::PinEvent::Input { .. }) => {}
                        Some(esp32bench_sim::gpio::PinEvent::Level { gpio, level, .. }) => {
                            eprintln!("[gpio {gpio} = {}]", u8::from(level));
                        }
                        Some(esp32bench_sim::gpio::PinEvent::Direction { gpio, output }) => {
                            eprintln!("[gpio {gpio} {}]", if output { "output" } else { "input" });
                        }
                        Some(esp32bench_sim::gpio::PinEvent::Part { id, state }) => {
                            eprintln!("[{id} {state}]");
                        }
                        Some(esp32bench_sim::gpio::PinEvent::PartError { id, message }) => {
                            eprintln!("[{id} error: {message}]");
                        }
                        Some(esp32bench_sim::gpio::PinEvent::Picture { id, width, height, pixels }) => {
                            let lit = pixels.iter().filter(|p| **p != 0).count();
                            match &frames {
                                Some(dir) => {
                                    // A part's picture as PGM: lit pixels white.
                                    saved += 1;
                                    let path = dir.join(format!("{id}-{saved:04}.pgm"));
                                    let mut pgm = format!("P5\n{width} {height}\n255\n").into_bytes();
                                    pgm.extend(pixels.iter().map(|p| if *p != 0 { 255u8 } else { 0 }));
                                    std::fs::write(&path, pgm)?;
                                    eprintln!("[{id} picture {width}x{height}, {lit} pixels lit: {}]", path.display());
                                }
                                None => eprintln!("[{id} picture {width}x{height}, {lit} pixels lit]"),
                            }
                        }
                        None => gpio = None,
                    },
                    () = tokio::time::sleep(display.interval()) => {
                        if let Some(f) = display.poll(&mut s.qmp).await?
                            && let Some(dir) = &frames
                        {
                            saved += 1;
                            let path = dir.join(format!("frame-{saved:04}.ppm"));
                            let mut ppm = format!("P6\n{} {}\n255\n", f.width, f.height).into_bytes();
                            ppm.extend_from_slice(&f.rgb);
                            std::fs::write(&path, ppm)?;
                            eprintln!("saved {}", path.display());
                        }
                    }
                    chunk = s.uart.read_chunk() => match chunk? {
                        Some(bytes) => {
                            out.write_all(&bytes)?;
                            out.flush()?;
                            if let Some(lines) = joiner.as_mut().and_then(|j| j.feed(&bytes)) {
                                eprintln!(
                                    "[joining the simulated Wi-Fi: setting the sketch's network to {}, no password]",
                                    esp32bench_sim::network::SSID
                                );
                                s.uart.write(&lines).await?;
                            }
                        }
                        None => break,
                    },
                    line = stdin.next_line(), if stdin_open => match line? {
                        // :press <id>, :release <id>, :control <id> <name> <value>
                        Some(l) if l.starts_with(":press ") || l.starts_with(":release ") || l.starts_with(":control ") => {
                            let words: Vec<&str> = l.split_whitespace().collect();
                            let control = match words.as_slice() {
                                [":press", id] => Some((*id, "press", serde_json::json!(true))),
                                [":release", id] => Some((*id, "press", serde_json::json!(false))),
                                [":control", id, name, value] => serde_json::from_str::<serde_json::Value>(value)
                                    .ok()
                                    .filter(|v| v.is_boolean() || v.is_number())
                                    .map(|v| (*id, *name, v)),
                                _ => None,
                            };
                            match (control, gpio.as_mut().filter(|_| has_parts)) {
                                (Some((id, name, value)), Some(link)) => link.control(id, name, &value).await?,
                                (_, None) => eprintln!("no circuit; use --circuit"),
                                (None, _) => eprintln!("usage: :press <id>, :release <id>, :control <id> <name> <true|false|number>"),
                            }
                        }
                        Some(l) => match l.strip_prefix(":set ") {
                            Some(cmd) => {
                                let mut it = cmd.split_whitespace();
                                let parsed = (it.next().and_then(|g| g.parse::<u8>().ok()), it.next());
                                match (parsed, gpio.as_mut()) {
                                    ((Some(g), Some(v @ ("0" | "1"))), Some(link)) => link.set(g, v == "1").await?,
                                    (_, None) => eprintln!("no pins bridged; use --pins"),
                                    _ => eprintln!("usage: :set <gpio> <0|1>"),
                                }
                            }
                            None => s.uart.write(format!("{l}\r\n").as_bytes()).await?,
                        },
                        None => stdin_open = false,
                    },
                    _ = &mut ctrl_c => break,
                }
            }
            s.shutdown().await?;
            if let (Some(path), Some(r)) = (&trace, &recorder) {
                std::fs::write(path, r.to_vcd(env!("CARGO_PKG_VERSION")))
                    .with_context(|| format!("writing {}", path.display()))?;
                eprintln!(
                    "trace: {} pin changes written to {}",
                    r.len(),
                    path.display()
                );
            }
        }
        Cmd::Sim(SimCmd::BootTest {
            session,
            expect,
            timeout,
            reset,
            quiet,
        }) => {
            return boot_test(session, expect, Duration::from_secs(timeout), reset, quiet).await;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn board_command(cmd: BoardCmd) -> Result<()> {
    match cmd {
        BoardCmd::Inspect { conn } => {
            let mut b = esp32bench_flash::Board::connect(&conn.options())?;
            let info = b.info()?;
            println!(
                "chip      {} {}",
                info.chip,
                info.revision.as_deref().unwrap_or("")
            );
            println!("crystal   {} MHz", info.crystal_mhz);
            println!("flash     {} MiB", info.flash_bytes / (1024 * 1024));
            println!("features  {}", info.features.join(", "));
            if let Some(mac) = &info.mac {
                println!("mac       {mac}");
            }
            match &info.security {
                Some(s) => println!(
                    "security  secure boot {}, flash encryption {}, secure download {}",
                    on_off(s.secure_boot),
                    on_off(s.flash_encryption),
                    on_off(s.secure_download)
                ),
                None => println!("security  not reported by this chip's ROM"),
            }
            let scratch = std::env::temp_dir();
            match b.partitions(&scratch) {
                Ok(parts) => {
                    println!("\npartitions at 0x8000:");
                    for p in parts {
                        println!(
                            "  {:<12} {:<4} {:<9} 0x{:06x}  {:>6} KiB{}",
                            p.label,
                            p.kind,
                            p.subtype,
                            p.offset,
                            p.size / 1024,
                            if p.encrypted { "  encrypted" } else { "" }
                        );
                    }
                }
                Err(e) => println!("\npartitions: {e}"),
            }
            b.run()?;
        }
        BoardCmd::Flash { conn, image, whole } => {
            let data =
                std::fs::read(&image).with_context(|| format!("reading {}", image.display()))?;
            let mut b = esp32bench_flash::Board::connect(&conn.options())?;
            let info = b.info()?;
            if let Some(chip) = esp32bench_flash::image_chip(&data)
                && chip != info.chip
            {
                bail!(
                    "the board is an {}, but {} is for {chip}",
                    info.chip,
                    image.display()
                );
            }
            let part = if whole {
                &data[..]
            } else {
                esp32bench_flash::trim_erased_tail(&data)
            };
            eprintln!(
                "writing {} bytes to {} ({})",
                part.len(),
                conn.port.display(),
                info.chip
            );
            b.write(0, part, &mut |p| {
                if let esp32bench_flash::Progress::Writing { written, total, .. } = p {
                    eprint!("\r{:>3}%", written * 100 / total.max(1));
                }
            })?;
            eprintln!("\rdone, resetting the board");
            b.run()?;
        }
        BoardCmd::Read { conn, out, size } => {
            let mut b = esp32bench_flash::Board::connect(&conn.options())?;
            let info = b.info()?;
            let len = match size {
                Some(n) => n,
                None => u32::try_from(info.flash_bytes).context("flash too large")?,
            };
            eprintln!(
                "reading {len} bytes from {} ({})",
                conn.port.display(),
                info.chip
            );
            b.read(0, len, &out)?;
            eprintln!("saved {}", out.display());
            b.run()?;
        }
        BoardCmd::Erase { conn, yes } => {
            if !yes {
                bail!(
                    "erasing removes everything on the board, settings included; pass --yes to do it"
                );
            }
            let mut b = esp32bench_flash::Board::connect(&conn.options())?;
            eprintln!("erasing {}", conn.port.display());
            b.erase_all()?;
            b.run()?;
            eprintln!("done");
        }
    }
    Ok(())
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Print ports as a table, or as one JSON object (`{"ports": [...]}`) on one line.
fn print_ports(ports: &[esp32bench_device::Port], json: bool, pretty: bool) -> Result<()> {
    if json {
        let v = serde_json::json!({ "ports": ports });
        if pretty {
            println!("{}", serde_json::to_string_pretty(&v)?);
        } else {
            println!("{}", serde_json::to_string(&v)?);
        }
        return Ok(());
    }
    if ports.is_empty() {
        println!("no serial ports found");
    }
    for p in ports {
        let ids = p
            .usb
            .as_ref()
            .map(|u| format!("{:04x}:{:04x}", u.vid, u.pid))
            .unwrap_or_else(|| "-".into());
        let bridge = p
            .bridge
            .and_then(|b| serde_json::to_value(b).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into());
        let chips = if p.chips.is_empty() {
            "any".to_owned()
        } else {
            p.chips.join(",")
        };
        let access = match p.access {
            esp32bench_device::Access::Ok => "",
            esp32bench_device::Access::Denied => {
                "  (no access: check the udev rule and uucp group)"
            }
        };
        println!(
            "{:<16} {ids:<10} {bridge:<16} chips: {chips}{access}",
            p.path
        );
    }
    Ok(())
}

async fn start(a: SessionArgs, start_paused: bool) -> Result<Session> {
    let mut config = match a.qemu {
        // An explicit binary: run it as given.
        Some(qemu) => SessionConfig {
            chip: a.chip,
            qemu,
            image: a.image,
            gdb_port: None,
            start_paused: false,
            machine: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            bridge: Vec::new(),
            circuit: None,
            efuse: None,
            usb_console: false,
            network: None,
            icount: false,
        },
        None => {
            let launch = esp32bench_sim::engine::launch(a.chip, a.engine)?;
            eprintln!("engine: {}", launch.engine.as_str());
            if (!a.pins.is_empty() || a.circuit.is_some()) && !launch.hooks {
                bail!(
                    "--pins needs lcgamboa's QEMU: Espressif's has no pin hooks (install \
                     the fork with `esp32bench toolchain install qemu-lcgamboa`)"
                );
            }
            if a.usb_console && !launch.usb_console {
                bail!("--usb-console needs an ESP32-C3 on lcgamboa's QEMU");
            }
            let mut config = launch.config(a.chip, a.image);
            config.bridge = a.pins;
            config.circuit = a.circuit;
            config.usb_console = a.usb_console;
            if launch.network && !a.no_network {
                config.network = Some(esp32bench_sim::network::Network::on_a_free_port()?);
            }
            config
        }
    };
    config.gdb_port = a.gdb;
    config.start_paused = start_paused;
    config.icount = a.icount;
    if a.machine.is_some() {
        config.machine = a.machine;
    }
    config
        .extra_args
        .extend(a.qemu_args.into_iter().map(Into::into));
    let s = Session::start(config).await?;
    Ok(s)
}

async fn boot_test(
    session: SessionArgs,
    expect: String,
    timeout: Duration,
    reset: bool,
    quiet: bool,
) -> Result<ExitCode> {
    // With a GDB port, start halted so the stub is probed before the first instruction.
    let paused = session.gdb.is_some();
    let mut s = start(session, paused).await?;
    if let Some(addr) = s.gdb_addr() {
        let reply = gdb::probe(addr, Duration::from_secs(5)).await?;
        eprintln!("[boot-test] GDB stub at {addr} answered `{reply}`");
        s.resume().await?;
    }

    let rounds = if reset { 2 } else { 1 };
    let mut passed = true;
    for round in 0..rounds {
        if round > 0 {
            eprintln!("[boot-test] resetting over QMP");
            s.reset().await?;
        }
        let mut out = std::io::stdout();
        let outcome = s
            .uart
            .wait_for(expect.as_bytes(), timeout, |b| {
                if !quiet {
                    let _ = out.write_all(b);
                    let _ = out.flush();
                }
            })
            .await?;
        match outcome {
            WaitOutcome::Found => eprintln!("\n[boot-test] found `{expect}`"),
            WaitOutcome::Panic(report) => {
                // The report was already echoed with the rest of UART0 unless --quiet.
                if quiet {
                    eprintln!("{report}");
                }
                eprintln!("\n[boot-test] FAIL: firmware panicked");
                passed = false;
                break;
            }
            WaitOutcome::Timeout => {
                eprintln!(
                    "\n[boot-test] FAIL: `{expect}` not seen within {}s",
                    timeout.as_secs()
                );
                passed = false;
                break;
            }
            WaitOutcome::Closed => {
                eprintln!("\n[boot-test] FAIL: QEMU closed UART0");
                passed = false;
                break;
            }
        }
    }
    s.shutdown().await?;
    eprintln!("[boot-test] {}", if passed { "PASS" } else { "FAIL" });
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
