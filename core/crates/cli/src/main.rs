//! `esp32d`: command-line access to the esp32-desktop core.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use esp32d_sim::uart::WaitOutcome;
use esp32d_sim::{Arch, Chip, Session, SessionConfig, gdb, image, install, pin};

#[derive(Parser)]
#[command(name = "esp32d", version, about = "esp32-desktop core command line")]
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

#[derive(Subcommand)]
enum SimCmd {
    /// Boot an image and stream UART0 to stdout until QEMU exits or Ctrl-C.
    Run {
        #[arg(long)]
        chip: Chip,
        image: PathBuf,
        /// Open a GDB stub on 127.0.0.1 at this port.
        #[arg(long)]
        gdb: Option<u16>,
        /// Stay halted until a debugger continues the target.
        #[arg(long, requires = "gdb")]
        paused: bool,
    },
    /// Boot an image and pass if a marker appears on UART0 before the timeout.
    BootTest {
        #[arg(long)]
        chip: Chip,
        image: PathBuf,
        /// Text that must appear on UART0. The ROM prints `rst:0x` on every chip.
        #[arg(long, default_value = "rst:0x")]
        expect: String,
        /// Seconds to wait for the marker.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// After the marker, reset over QMP and require the marker again.
        #[arg(long)]
        reset: bool,
        /// Before boot, check the GDB stub on this port answers.
        #[arg(long)]
        gdb: Option<u16>,
        /// Do not echo UART output.
        #[arg(long, short)]
        quiet: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
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
        Cmd::Sim(SimCmd::Run {
            chip,
            image,
            gdb,
            paused,
        }) => {
            let mut s = start(chip, image, gdb, paused).await?;
            if let Some(addr) = s.gdb_addr() {
                eprintln!("GDB stub on {addr}");
            }
            let mut out = std::io::stdout();
            loop {
                tokio::select! {
                    chunk = s.uart.read_chunk() => match chunk? {
                        Some(bytes) => { out.write_all(&bytes)?; out.flush()?; }
                        None => break,
                    },
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            s.shutdown().await?;
        }
        Cmd::Sim(SimCmd::BootTest {
            chip,
            image,
            expect,
            timeout,
            reset,
            gdb,
            quiet,
        }) => {
            return boot_test(
                chip,
                image,
                expect,
                Duration::from_secs(timeout),
                reset,
                gdb,
                quiet,
            )
            .await;
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn start(
    chip: Chip,
    image: PathBuf,
    gdb_port: Option<u16>,
    start_paused: bool,
) -> Result<Session> {
    let qemu = install::find_qemu(chip.arch())?;
    let s = Session::start(SessionConfig {
        chip,
        qemu,
        image,
        gdb_port,
        start_paused,
    })
    .await?;
    Ok(s)
}

async fn boot_test(
    chip: Chip,
    image: PathBuf,
    expect: String,
    timeout: Duration,
    reset: bool,
    gdb_port: Option<u16>,
    quiet: bool,
) -> Result<ExitCode> {
    let mut s = start(chip, image, gdb_port, gdb_port.is_some()).await?;
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
