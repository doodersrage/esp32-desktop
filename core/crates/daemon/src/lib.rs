//! The core daemon's method table and socket setup. `main.rs` only parses arguments
//! and handles signals; everything here is testable without a running daemon.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use esp32bench_rpc::{Ctx, Router, RpcError, code, parse_params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::UnixListener;

/// Protocol version, as in `schema/rpc/index.json`. A test keeps them equal.
pub const PROTOCOL: &str = "0.8.0";
pub const NAME: &str = "esp32benchd";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Daemon-wide state shared by all connections.
pub struct State {
    /// Project library cache.
    pub library: esp32bench_library::Cache,
    /// Where library.refresh fetches from.
    pub library_source: String,
    /// One refresh at a time, so two never race on the cache file.
    refresh_lock: tokio::sync::Mutex<()>,
    /// Consent tokens handed out for builds (rule 4).
    pub consent: esp32bench_build::Consent,
    /// Build folders live under here, one per project and board.
    pub builds_root: PathBuf,
    /// Private Arduino sketchbook for installed libraries.
    pub sketchbook: PathBuf,
    /// arduino-cli to use instead of `$ESP32BENCH_ARDUINO_CLI` or PATH (tests).
    pub arduino_cli: Option<PathBuf>,
    /// One build at a time.
    build_lock: tokio::sync::Mutex<()>,
    /// Open serial ports by (connection, subscription): subscription ids are only
    /// unique within a connection.
    serial_ports: std::sync::Mutex<HashMap<(u64, u64), SharedPort>>,
    /// Ports serial.open may open instead of those device.list reports (tests).
    pub serial_allow: Option<Vec<PathBuf>>,
    /// Arduino's data directory (~/.arduino15), for package index files.
    pub arduino_data: PathBuf,
    /// Consent tokens handed out for flashing downloaded projects (rule 4).
    pub flash_consent: esp32bench_build::Consent<FlashPlan>,
    /// One board operation at a time.
    board_lock: tokio::sync::Mutex<()>,
}

/// What board.flash will write, shown to the user before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlashPlan {
    pub project: String,
    pub board: String,
    pub port: PathBuf,
    pub image_sha256: String,
    pub bytes: usize,
}

type SharedPort = Arc<std::sync::Mutex<esp32bench_serial::Port>>;

/// Removes a serial port from the table when its subscription ends, however it ends,
/// which drops the Port and closes the device.
struct PortEntry {
    state: Arc<State>,
    key: (u64, u64),
}

impl Drop for PortEntry {
    fn drop(&mut self) {
        let mut ports = self
            .state
            .serial_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        ports.remove(&self.key);
    }
}

impl State {
    pub fn new(
        library: esp32bench_library::Cache,
        library_source: String,
        builds_root: PathBuf,
        sketchbook: PathBuf,
    ) -> Self {
        State {
            library,
            library_source,
            refresh_lock: tokio::sync::Mutex::new(()),
            consent: esp32bench_build::Consent::new(),
            builds_root,
            sketchbook,
            arduino_cli: None,
            build_lock: tokio::sync::Mutex::new(()),
            serial_ports: std::sync::Mutex::new(HashMap::new()),
            serial_allow: None,
            arduino_data: esp32bench_toolchain::Env::default_arduino_data()
                .unwrap_or_else(|| PathBuf::from(".arduino15")),
            flash_consent: esp32bench_build::Consent::new(),
            board_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Cache in `$XDG_CACHE_HOME/esp32bench`; source from `$ESP32BENCH_MANIFEST` or the site.
    pub fn from_env() -> io::Result<Self> {
        let dir = esp32bench_library::cache_dir()
            .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?;
        let sketchbook = esp32bench_build::default_sketchbook().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no data directory: set XDG_DATA_HOME or HOME",
            )
        })?;
        Ok(Self::new(
            esp32bench_library::Cache::new(dir.clone()),
            esp32bench_library::default_source(),
            dir.join("builds"),
            sketchbook,
        ))
    }
}

/// Every method the daemon serves. A test checks this against `schema/rpc/`.
pub fn router(state: Arc<State>) -> Router {
    let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
    let (s5, s6, s7) = (state.clone(), state.clone(), state.clone());
    let (s8, s9) = (state.clone(), state.clone());
    let (s10, s11, s12) = (state.clone(), state.clone(), state);
    Router::new()
        .method("board.inspect", move |p, ctx| {
            let s = s10.clone();
            async move { board_inspect(p, ctx, s).await }
        })
        .method("board.flash", move |p, ctx| {
            let s = s11.clone();
            async move { board_flash(p, ctx, s).await }
        })
        .method("board.erase", move |p, ctx| {
            let s = s12.clone();
            async move { board_erase(p, ctx, s).await }
        })
        .method("toolchain.status", move |p, _| {
            let s = s8.clone();
            async move { toolchain_status(p, s).await }
        })
        .method("toolchain.install", move |p, ctx| {
            let s = s9.clone();
            async move { toolchain_install(p, ctx, s) }
        })
        .method("serial.open", move |p, ctx| {
            let s = s5.clone();
            async move { serial_open(p, ctx, s).await }
        })
        .method("serial.write", move |p, ctx| {
            let s = s6.clone();
            async move { serial_write(p, ctx, s).await }
        })
        .method("serial.reset", move |p, ctx| {
            let s = s7.clone();
            async move { serial_reset(p, ctx, s).await }
        })
        .method("build.start", move |p, ctx| {
            let s = s4.clone();
            async move { build_start(p, ctx, s).await }
        })
        .method("core.hello", |p, _| async move { core_hello(p) })
        .method("job.cancel", |p, ctx| async move { job_cancel(p, ctx) })
        .method("library.list", move |p, _| {
            let s = s1.clone();
            async move { library_list(p, s).await }
        })
        .method("library.get", move |p, _| {
            let s = s3.clone();
            async move { library_get(p, s).await }
        })
        .method("library.refresh", move |p, ctx| {
            let s = s2.clone();
            async move { library_refresh(p, ctx, s) }
        })
        .method("core.unsubscribe", |p, ctx| async move {
            core_unsubscribe(p, ctx)
        })
        .method("device.list", |p, _| async move { device_list(p).await })
        .method("device.watch", |p, ctx| async move {
            device_watch(p, ctx).await
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloParams {
    #[serde(default)]
    #[allow(dead_code)] // logged once there is logging
    client: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    protocol: Option<String>,
}

fn core_hello(params: Value) -> Result<Value, RpcError> {
    let _: HelloParams = parse_params(params)?;
    Ok(json!({ "name": NAME, "version": VERSION, "protocol": PROTOCOL }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsubscribeParams {
    subscription: u64,
}

fn core_unsubscribe(params: Value, ctx: Ctx) -> Result<Value, RpcError> {
    let p: UnsubscribeParams = parse_params(params)?;
    if ctx.unsubscribe(p.subscription) {
        Ok(json!({}))
    } else {
        Err(RpcError::new(
            code::NOT_FOUND,
            format!("no subscription {} on this connection", p.subscription),
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobParams {
    job: u64,
}

/// Jobs are subscriptions that end themselves; cancelling one aborts its task.
fn job_cancel(params: Value, ctx: Ctx) -> Result<Value, RpcError> {
    let p: JobParams = parse_params(params)?;
    if ctx.unsubscribe(p.job) {
        Ok(json!({}))
    } else {
        Err(RpcError::new(
            code::NOT_FOUND,
            format!("no job {} on this connection", p.job),
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

async fn library_list(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let _: NoParams = parse_params(params)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    let Some(m) = manifest else {
        return Ok(json!({ "available": false, "source": state.library_source, "projects": [] }));
    };
    let mut out = json!({
        "available": true,
        "source": state.library_source,
        "version": m.version,
        "generated": m.generated,
        "projects": m.summaries(),
    });
    if let Some(t) = esp32bench_library::modified_secs(&state.library.path()) {
        out["fetchedAt"] = json!(t);
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    id: String,
}

async fn library_get(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: GetParams = parse_params(params)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                "the project library has not been downloaded",
            )
        })?;
    let project = manifest.project(&p.id).ok_or_else(|| {
        RpcError::new(
            code::NOT_FOUND,
            format!("no project `{}` in the library", p.id),
        )
    })?;
    let boards: Vec<_> = manifest
        .boards
        .iter()
        .filter(|b| project.builds.iter().any(|x| x.board == b.id))
        .collect();
    Ok(json!({ "project": project, "boards": boards }))
}

fn library_refresh(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let _: NoParams = parse_params(params)?;
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.refresh_lock.lock().await;
        let event = match state.library.refresh(&state.library_source).await {
            Ok(m) => json!({ "ok": true, "projects": m.projects.len(), "version": m.version }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        };
        sub.send("library.refreshed", event);
    });
    Ok(json!({ "job": job }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildParams {
    project: String,
    board: String,
    #[serde(default)]
    consent: Option<String>,
}

fn build_error(e: esp32bench_build::Error) -> RpcError {
    use esp32bench_build::Error as E;
    let code = match &e {
        E::NoProject(_) | E::NoBuild { .. } => code::NOT_FOUND,
        E::BadId(_) => code::INVALID_PARAMS,
        E::NoArduinoCli => code::UNAVAILABLE,
        _ => code::FAILED,
    };
    RpcError::new(code, e.to_string())
}

async fn build_start(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: BuildParams = parse_params(params)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                "the project library has not been downloaded",
            )
        })?;
    let (plan, code) =
        esp32bench_build::plan(&manifest, &p.project, &p.board).map_err(build_error)?;
    // Check the toolchain before asking for consent, so nobody agrees to a build that
    // cannot run.
    let tools = match &state.arduino_cli {
        Some(cli) => esp32bench_build::Toolchain {
            arduino_cli: cli.clone(),
            sketchbook: state.sketchbook.clone(),
        },
        None => {
            esp32bench_build::Toolchain::detect(state.sketchbook.clone()).map_err(build_error)?
        }
    };

    // Library projects are downloaded, so they never build without consent (rule 4).
    let granted = p
        .consent
        .as_deref()
        .is_some_and(|t| state.consent.redeem(t, &plan));
    if !granted {
        let token = state
            .consent
            .issue(&plan)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let message = if p.consent.is_some() {
            "the consent token is not valid for this build; ask the user again"
        } else {
            "this project was downloaded; ask the user before building it"
        };
        return Err(RpcError::new(code::CONSENT_REQUIRED, message)
            .with_data(json!({ "token": token, "plan": plan })));
    }

    let result_plan = plan.clone();
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.build_lock.lock().await;
        let dirs = esp32bench_build::Dirs::new(&state.builds_root, &plan);
        let r = esp32bench_build::run(&plan, &code, &tools, &dirs, |stream, line| {
            sub.send("build.log", json!({ "stream": stream, "line": line }));
        })
        .await;
        let event = match r {
            Ok(out) => json!({ "ok": true, "image": out.image, "elf": out.elf }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        };
        sub.send("build.finished", event);
    });
    Ok(json!({ "job": job, "plan": result_plan }))
}

fn toolchain_env(state: &State) -> esp32bench_toolchain::Env {
    esp32bench_toolchain::Env {
        arduino_cli: state.arduino_cli.clone(),
        arduino_data: state.arduino_data.clone(),
        sketchbook: state.sketchbook.clone(),
    }
}

async fn toolchain_status(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let _: NoParams = parse_params(params)?;
    let components = esp32bench_toolchain::status(&toolchain_env(&state)).await;
    Ok(json!({ "components": components }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallParams {
    component: String,
}

fn toolchain_install(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: InstallParams = parse_params(params)?;
    let id = esp32bench_toolchain::Id::parse(&p.component)
        .ok_or_else(|| RpcError::invalid_params(format!("unknown component `{}`", p.component)))?;
    if id == esp32bench_toolchain::Id::ArduinoCli {
        return Err(RpcError::new(
            code::FAILED,
            "arduino-cli is a system package; install it with: sudo pacman -S arduino-cli",
        ));
    }
    let job = ctx.subscribe(move |sub| async move {
        // Installs share the build lock: never swap the core out under a build.
        let _one_at_a_time = state.build_lock.lock().await;
        let env = toolchain_env(&state);
        let r = esp32bench_toolchain::install(&env, id, |line| {
            sub.send("toolchain.progress", json!({ "line": line }));
        })
        .await;
        let event = match r {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        };
        sub.send("toolchain.finished", event);
    });
    Ok(json!({ "job": job }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialOpenParams {
    path: PathBuf,
    baud: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialWriteParams {
    subscription: u64,
    data: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialRefParams {
    subscription: u64,
}

/// Port paths are untrusted: only use something device.list reports as a serial
/// port, never an arbitrary file or device node.
async fn check_port(state: &State, path: &Path) -> Result<(), RpcError> {
    let allowed = match &state.serial_allow {
        Some(list) => list.iter().any(|p| p == path),
        None => list_ports(true)
            .await?
            .iter()
            .any(|x| Path::new(&x.path) == path),
    };
    if allowed {
        Ok(())
    } else {
        Err(RpcError::new(
            code::NOT_FOUND,
            format!(
                "{} is not a serial port this machine reports",
                path.display()
            ),
        ))
    }
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
enum ResetParam {
    #[default]
    Auto,
    None,
}

impl ResetParam {
    fn reset(self) -> esp32bench_flash::Reset {
        match self {
            ResetParam::Auto => esp32bench_flash::Reset::Auto,
            ResetParam::None => esp32bench_flash::Reset::None,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardParams {
    port: PathBuf,
    #[serde(default)]
    reset: ResetParam,
    #[serde(default = "yes")]
    stub: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EraseParams {
    port: PathBuf,
    #[serde(default)]
    reset: ResetParam,
    #[serde(default = "yes")]
    stub: bool,
    confirm: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlashParams {
    port: PathBuf,
    #[serde(default)]
    reset: ResetParam,
    #[serde(default = "yes")]
    stub: bool,
    project: String,
    board: String,
    #[serde(default)]
    consent: Option<String>,
}

fn board_options(port: &Path, reset: ResetParam, stub: bool) -> esp32bench_flash::Options {
    esp32bench_flash::Options {
        port: port.to_owned(),
        reset: reset.reset(),
        use_stub: stub,
    }
}

/// What a board job does once connected.
enum BoardOp {
    Inspect { scratch: PathBuf },
    Flash { image: Vec<u8>, chip: String },
    Erase,
}

/// Sets a flag when dropped: the job's task was aborted (job.cancel or disconnect).
struct SetOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Run `op` on a blocking thread, reporting stages as board.progress and the outcome
/// as board.finished. espflash cannot be interrupted mid-operation, so job.cancel
/// takes effect between steps, never in the middle of a write.
fn board_job(ctx: &Ctx, state: Arc<State>, opts: esp32bench_flash::Options, op: BoardOp) -> u64 {
    ctx.subscribe(move |sub| {
        let sub = Arc::new(sub);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = SetOnDrop(cancelled.clone());
        async move {
            let _flag = flag;
            let _one_at_a_time = state.board_lock.lock().await;
            let thread_sub = sub.clone();
            let r = tokio::task::spawn_blocking(move || {
                run_board_op(&opts, op, &cancelled, &thread_sub)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let event = match r {
                Ok(mut done) => {
                    done["ok"] = json!(true);
                    done
                }
                Err(e) => json!({ "ok": false, "error": e }),
            };
            sub.send("board.finished", event);
        }
    })
}

fn run_board_op(
    opts: &esp32bench_flash::Options,
    op: BoardOp,
    cancelled: &std::sync::atomic::AtomicBool,
    sub: &esp32bench_rpc::Subscription,
) -> Result<Value, String> {
    let stage = |name: &str| {
        sub.send("board.progress", json!({ "stage": name }));
    };
    let check = || {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            Err("cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    stage("connecting");
    let mut board = esp32bench_flash::Board::connect(opts).map_err(|e| e.to_string())?;
    check()?;
    let mut out = json!({});
    match op {
        BoardOp::Inspect { scratch } => {
            stage("reading");
            let info = board.info().map_err(|e| e.to_string())?;
            out["info"] = json!(info);
            std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
            // An erased board has no table; report the chip anyway.
            if let Ok(parts) = board.partitions(&scratch) {
                out["partitions"] = json!(parts);
            }
        }
        BoardOp::Flash { image, chip } => {
            stage("reading");
            let info = board.info().map_err(|e| e.to_string())?;
            if info.chip != chip {
                return Err(format!(
                    "the board is an {}, but this build is for {chip}",
                    info.chip
                ));
            }
            check()?;
            let data = esp32bench_flash::trim_erased_tail(&image);
            board
                .write(0, data, &mut |p| match p {
                    esp32bench_flash::Progress::Writing { written, total, .. } => {
                        sub.send(
                            "board.progress",
                            json!({ "stage": "writing", "written": written, "total": total }),
                        );
                    }
                    esp32bench_flash::Progress::Verifying => {
                        sub.send("board.progress", json!({ "stage": "verifying" }));
                    }
                })
                .map_err(|e| e.to_string())?;
            out["written"] = json!(data.len());
        }
        BoardOp::Erase => {
            // The erase commands exist only in espflash's stub, not in the ROM.
            if !opts.use_stub {
                return Err("erasing needs espflash's stub loader, which this connection does not use (QEMU cannot run it)".to_owned());
            }
            stage("erasing");
            board.erase_all().map_err(|e| e.to_string())?;
        }
    }
    stage("resetting");
    board.run().map_err(|e| e.to_string())?;
    Ok(out)
}

async fn board_inspect(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: BoardParams = parse_params(params)?;
    check_port(&state, &p.port).await?;
    let scratch = state.builds_root.join(".scratch");
    let opts = board_options(&p.port, p.reset, p.stub);
    let job = board_job(&ctx, state, opts, BoardOp::Inspect { scratch });
    Ok(json!({ "job": job }))
}

async fn board_erase(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: EraseParams = parse_params(params)?;
    if !p.confirm {
        return Err(RpcError::invalid_params("erasing needs confirm: true"));
    }
    check_port(&state, &p.port).await?;
    let opts = board_options(&p.port, p.reset, p.stub);
    let job = board_job(&ctx, state, opts, BoardOp::Erase);
    Ok(json!({ "job": job }))
}

async fn board_flash(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    use sha2::{Digest, Sha256};
    let p: FlashParams = parse_params(params)?;
    check_port(&state, &p.port).await?;
    for id in [&p.project, &p.board] {
        esp32bench_build::safe_id(id).map_err(|e| RpcError::invalid_params(e.to_string()))?;
    }
    // The chip the build is for comes from the library, not from the caller.
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                "the project library has not been downloaded",
            )
        })?;
    let chip = manifest
        .boards
        .iter()
        .find(|b| b.id == p.board)
        .map(|b| b.chip.clone())
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                format!("no board `{}` in the library", p.board),
            )
        })?;
    let image_path = state
        .builds_root
        .join(&p.project)
        .join(&p.board)
        .join("out")
        .join(format!("{}.ino.merged.bin", p.project));
    let image = tokio::fs::read(&image_path).await.map_err(|_| {
        RpcError::new(
            code::NOT_FOUND,
            format!("`{}` has not been built for `{}` yet", p.project, p.board),
        )
    })?;
    let plan = FlashPlan {
        project: p.project.clone(),
        board: p.board.clone(),
        port: p.port.clone(),
        image_sha256: Sha256::digest(&image)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        bytes: esp32bench_flash::trim_erased_tail(&image).len(),
    };
    // Flashing downloaded firmware runs it; ask first (rule 4).
    let granted = p
        .consent
        .as_deref()
        .is_some_and(|t| state.flash_consent.redeem(t, &plan));
    if !granted {
        let token = state
            .flash_consent
            .issue(&plan)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        return Err(RpcError::new(
            code::CONSENT_REQUIRED,
            "this project was downloaded; ask the user before flashing it",
        )
        .with_data(json!({ "token": token, "plan": plan })));
    }
    let opts = board_options(&p.port, p.reset, p.stub);
    let job = board_job(&ctx, state, opts, BoardOp::Flash { image, chip });
    Ok(json!({ "job": job }))
}

fn serial_error(e: esp32bench_serial::Error) -> RpcError {
    use esp32bench_serial::Error as E;
    let code = match &e {
        E::Baud(_) => code::INVALID_PARAMS,
        E::Busy(_) => code::BUSY,
        E::NotFound(_) => code::NOT_FOUND,
        E::Permission(_) | E::Port { .. } => code::FAILED,
    };
    RpcError::new(code, e.to_string())
}

async fn serial_open(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialOpenParams = parse_params(params)?;
    check_port(&state, &p.path).await?;
    let (port, mut rx) = {
        let path = p.path.clone();
        tokio::task::spawn_blocking(move || esp32bench_serial::Port::open(&path, p.baud))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .map_err(serial_error)?
    };
    let port: SharedPort = Arc::new(std::sync::Mutex::new(port));
    let conn = ctx.connection_id();
    let id = ctx.subscribe(move |sub| {
        // Registered before the task can run, and removed by `entry` when the task
        // ends or is aborted, so the table never holds a closed subscription's port.
        let key = (conn, sub.id());
        state
            .serial_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, port);
        let entry = PortEntry {
            state: state.clone(),
            key,
        };
        async move {
            let _entry = entry;
            use base64::Engine;
            while let Some(event) = rx.recv().await {
                match event {
                    esp32bench_serial::Event::Data(bytes) => {
                        let sent = sub.send(
                            "serial.data",
                            json!({
                                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                                "text": String::from_utf8_lossy(&bytes),
                            }),
                        );
                        if !sent {
                            break;
                        }
                    }
                    esp32bench_serial::Event::Closed(reason) => {
                        sub.send("serial.closed", json!({ "reason": reason }));
                        break;
                    }
                }
            }
        }
    });
    Ok(json!({ "subscription": id }))
}

/// The port a connection opened under `subscription`.
fn open_port(state: &State, ctx: &Ctx, subscription: u64) -> Result<SharedPort, RpcError> {
    state
        .serial_ports
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(ctx.connection_id(), subscription))
        .cloned()
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                format!("no open serial port {subscription} on this connection"),
            )
        })
}

async fn serial_write(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    use base64::Engine;
    let p: SerialWriteParams = parse_params(params)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.data)
        .map_err(|e| RpcError::invalid_params(format!("data is not base64: {e}")))?;
    let port = open_port(&state, &ctx, p.subscription)?;
    tokio::task::spawn_blocking(move || {
        port.lock().unwrap_or_else(|p| p.into_inner()).write(&bytes)
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))?
    .map_err(serial_error)?;
    Ok(json!({}))
}

async fn serial_reset(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialRefParams = parse_params(params)?;
    let port = open_port(&state, &ctx, p.subscription)?;
    // The reset pulse sleeps for 100 ms; keep it off the async workers.
    tokio::task::spawn_blocking(move || port.lock().unwrap_or_else(|p| p.into_inner()).reset())
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(serial_error)?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceListParams {
    #[serde(default)]
    all: bool,
}

/// List ports off the async workers; udev enumeration blocks.
async fn list_ports(all: bool) -> Result<Vec<esp32bench_device::Port>, RpcError> {
    tokio::task::spawn_blocking(move || esp32bench_device::list(all))
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))
}

async fn device_list(params: Value) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    Ok(json!({ "ports": list_ports(p.all).await? }))
}

async fn device_watch(params: Value, ctx: Ctx) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    // Start monitoring before listing, so a change in between is not missed.
    let mut monitor = esp32bench_device::Monitor::new()
        .map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))?;
    let ports = list_ports(p.all).await?;
    let mut last = ports.clone();
    let id = ctx.subscribe(move |sub| async move {
        // Events go out only after the monitor's settle delay (300 ms), so in
        // practice the client has the subscription id before the first one arrives.
        while monitor.changed().await.is_ok() {
            let Ok(now) = list_ports(p.all).await else {
                continue;
            };
            if now == last {
                continue;
            }
            if !sub.send("device.changed", json!({ "ports": now })) {
                break;
            }
            last = now;
        }
    });
    Ok(json!({ "subscription": id, "ports": ports }))
}

/// Holds the daemon's lock file; the lock is released when this is dropped.
pub struct InstanceLock {
    _file: File,
    socket: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Take the single-instance lock next to `socket`, then bind the socket (mode 0600).
/// Fails if another daemon holds the lock. A stale socket left by a crashed daemon is
/// replaced, which is safe because the lock proves nobody is serving it.
pub fn bind(socket: &Path) -> io::Result<(UnixListener, InstanceLock)> {
    let dir = socket.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "socket path has no directory")
    })?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    let lock_path = dir.join("core.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    // SAFETY: flock on a file descriptor we own; non-blocking.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "another esp32benchd is running ({} is locked)",
                lock_path.display()
            ),
        ));
    }

    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok((
        listener,
        InstanceLock {
            _file: file,
            socket: socket.to_owned(),
        },
    ))
}
