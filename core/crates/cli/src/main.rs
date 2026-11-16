//! `esp32bench`: command-line access to the esp32-desktop core.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
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

#[tokio::main]
async fn main() -> ExitCode {
    // Exit quietly when stdout is a closed pipe (e.g. `| head`) instead of panicking.
    // SAFETY: setting a signal disposition to the default before any threads use it.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
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
            let ports = esp32bench_device::list(all)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "ports": ports }))?
                );
            } else if ports.is_empty() {
                println!("no serial ports found");
            } else {
                for p in &ports {
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
                    .context("XDG_RUNTIME_DIR is not set; pass --socket")?,
            };
            let mut client = esp32bench_rpc::Client::connect(&socket).await?;
            let result = client.call(&method, params).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Cmd::Sim(SimCmd::Run { session, paused }) => {
            if paused && session.gdb.is_none() {
                bail!("--paused needs --gdb, or nothing can resume the target");
            }
            let mut s = start(session, paused).await?;
            if let Some(addr) = s.gdb_addr() {
                eprintln!("GDB stub on {addr}");
            }
            let mut out = std::io::stdout();
            // Lines typed on stdin go to UART0 RX, ending in CR LF like a serial terminal.
            let mut stdin = BufReader::new(tokio::io::stdin()).lines();
            let mut stdin_open = true;
            loop {
                tokio::select! {
                    chunk = s.uart.read_chunk() => match chunk? {
                        Some(bytes) => { out.write_all(&bytes)?; out.flush()?; }
                        None => break,
                    },
                    line = stdin.next_line(), if stdin_open => match line? {
                        Some(l) => s.uart.write(format!("{l}\r\n").as_bytes()).await?,
                        None => stdin_open = false,
                    },
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            s.shutdown().await?;
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

async fn start(a: SessionArgs, start_paused: bool) -> Result<Session> {
    let qemu = match a.qemu {
        Some(q) => q,
        None => install::find_qemu(a.chip.arch())?,
    };
    let s = Session::start(SessionConfig {
        chip: a.chip,
        qemu,
        image: a.image,
        gdb_port: a.gdb,
        start_paused,
        machine: a.machine,
        extra_args: a.qemu_args.into_iter().map(Into::into).collect(),
        env: Vec::new(),
    })
    .await?;
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
