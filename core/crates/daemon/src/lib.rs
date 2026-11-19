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
pub const PROTOCOL: &str = "0.23.0";
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
    /// Where circuits (virtual parts per project and board) are saved.
    pub circuits_root: PathBuf,
    /// Running simulators by (connection, subscription), to send them commands.
    sims: std::sync::Mutex<HashMap<(u64, u64), tokio::sync::mpsc::UnboundedSender<SimCmd>>>,
    /// Signal traces of the latest simulations by (connection, subscription), kept
    /// after a session ends so sim.traceSave still works; at most TRACES_KEPT.
    traces: std::sync::Mutex<std::collections::VecDeque<((u64, u64), SharedTrace)>>,
    /// Local ESP-IDF projects the user opened (local.open).
    pub local: esp32bench_build::idf::Registry,
    /// ESP-IDF to use instead of detecting one (tests).
    pub idf_path: Option<PathBuf>,
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
type SharedTrace = Arc<std::sync::Mutex<esp32bench_sim::trace::Trace>>;

/// How many simulations' traces sim.traceSave can still reach.
const TRACES_KEPT: usize = 8;
/// Edges per sim.trace event at most; the rest are counted as `skipped` (all are
/// still recorded for sim.traceSave).
const TRACE_BATCH_MAX: usize = 4096;

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
            sims: std::sync::Mutex::new(HashMap::new()),
            traces: std::sync::Mutex::new(std::collections::VecDeque::new()),
            circuits_root: esp32bench_sim::install::data_dir()
                .map(|d| d.join("circuits"))
                .unwrap_or_else(|_| PathBuf::from("esp32bench-circuits")),
            board_lock: tokio::sync::Mutex::new(()),
            local: esp32bench_build::idf::Registry::new(
                esp32bench_sim::install::data_dir()
                    .map(|d| d.join("local-projects.json"))
                    .unwrap_or_else(|_| PathBuf::from("esp32bench-local-projects.json")),
            ),
            idf_path: None,
        }
    }

    /// The ESP-IDF install local builds use, if there is one.
    pub fn idf(&self) -> Option<esp32bench_build::idf::Idf> {
        match &self.idf_path {
            Some(p) => esp32bench_build::idf::at(p),
            None => esp32bench_build::idf::detect(),
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
    let local = state.clone();
    let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
    let (s5, s6, s7) = (state.clone(), state.clone(), state.clone());
    let links = state.clone();
    let (s8, s9) = (state.clone(), state.clone());
    let (s10, s11, s12) = (state.clone(), state.clone(), state.clone());
    let (s13, s14, s15) = (state.clone(), state.clone(), state.clone());
    let (d1, d2, d3, d4, d5) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    let (c1, c2, c3, c4) = (state.clone(), state.clone(), state.clone(), state);
    Router::new()
        .method("sim.start", move |p, ctx| {
            let s = s13.clone();
            async move { sim_start(p, ctx, s).await }
        })
        .method("sim.write", move |p, ctx| {
            let s = s14.clone();
            async move { sim_write(p, ctx, s).await }
        })
        .method("sim.reset", move |p, ctx| {
            let s = s15.clone();
            async move { sim_reset(p, ctx, s).await }
        })
        .method("sim.setPin", move |p, ctx| {
            let s = d5.clone();
            async move { sim_set_pin(p, ctx, s).await }
        })
        .method("sim.control", move |p, ctx| {
            let s = c1.clone();
            async move { sim_control(p, ctx, s).await }
        })
        .method("sim.traceSave", move |p, ctx| {
            let s = c4.clone();
            async move { sim_trace_save(p, ctx, s).await }
        })
        .method(
            "circuit.models",
            move |_p, _ctx| async move { circuit_models() },
        )
        .method("circuit.get", move |p, _ctx| {
            let s = c2.clone();
            async move { circuit_get(p, s).await }
        })
        .method("circuit.set", move |p, _ctx| {
            let s = c3.clone();
            async move { circuit_set(p, s).await }
        })
        .method("debug.break", move |p, ctx| {
            let s = d1.clone();
            async move { debug_break(p, ctx, s).await }
        })
        .method("debug.delete", move |p, ctx| {
            let s = d2.clone();
            async move { debug_delete(p, ctx, s).await }
        })
        .method("debug.control", move |p, ctx| {
            let s = d3.clone();
            async move { debug_control(p, ctx, s).await }
        })
        .method("debug.inspect", move |p, ctx| {
            let s = d4.clone();
            async move { debug_inspect(p, ctx, s).await }
        })
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
        .method("local.open", {
            let s = local.clone();
            move |p, _| {
                let s = s.clone();
                async move { local_open(p, s).await }
            }
        })
        .method("local.list", {
            let s = local.clone();
            move |p, _| {
                let s = s.clone();
                async move { local_list(p, s).await }
            }
        })
        .method("local.get", {
            let s = local.clone();
            move |p, _| {
                let s = s.clone();
                async move { local_get(p, s).await }
            }
        })
        .method("local.remove", {
            let s = local;
            move |p, _| {
                let s = s.clone();
                async move { local_remove(p, s).await }
            }
        })
        .method("link.open", {
            let s = links;
            move |p, _| {
                let s = s.clone();
                async move { link_open(p, s).await }
            }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkParams {
    url: String,
}

/// An `esp32bench://` link from the site: the project it names, if it is in the
/// library, and the board when that project has a build for it. Nothing is built or
/// flashed: the app selects the project, and building still asks for consent.
async fn link_open(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: LinkParams = parse_params(params)?;
    let link = esp32bench_library::link::parse(&p.url).map_err(RpcError::invalid_params)?;
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
    let project = manifest.project(&link.project).ok_or_else(|| {
        RpcError::new(
            code::NOT_FOUND,
            format!("no project `{}` in the library", link.project),
        )
    })?;
    let board = link
        .board
        .filter(|b| project.builds.iter().any(|x| &x.board == b));
    Ok(json!({ "project": project.id, "board": board }))
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
        E::NoArduinoCli | E::NoIdf => code::UNAVAILABLE,
        E::NoLocalProject(_) => code::NOT_FOUND,
        E::NotAnIdfProject(_) => code::INVALID_PARAMS,
        _ => code::FAILED,
    };
    RpcError::new(code, e.to_string())
}

async fn build_start(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: BuildParams = parse_params(params)?;
    if esp32bench_build::idf::is_local(&p.project) {
        return local_build_start(p, ctx, state);
    }
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

/// An ESP-IDF build of a local project. The user opened the folder, so no consent is
/// asked for (rule 4 is about downloaded projects); the events are build.start's.
fn local_build_start(p: BuildParams, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    use esp32bench_build::{Error as E, idf};
    esp32bench_build::safe_id(&p.board).map_err(build_error)?;
    let project = state
        .local
        .get(&p.project)
        .ok_or_else(|| build_error(E::NoLocalProject(p.project.clone())))?;
    let install = state.idf().ok_or_else(|| build_error(E::NoIdf))?;
    let plan = idf::plan(&project, &p.board, &install).map_err(build_error)?;
    let result_plan = plan.clone();
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.build_lock.lock().await;
        let dirs = idf::Dirs::new(&state.builds_root, &plan);
        let r = idf::run(&plan, &dirs, |stream, line| {
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

/// The boards a local project builds for, named as the library names them when it is
/// there, and their chips.
async fn local_boards(state: &State) -> Vec<Value> {
    let manifest = state.library.load().await.ok().flatten();
    esp32bench_build::idf::TARGETS
        .iter()
        .map(|(board, chip)| {
            let name = manifest
                .as_ref()
                .and_then(|m| m.boards.iter().find(|b| b.id == *board))
                .map(|b| b.name.clone())
                .unwrap_or_else(|| (*board).to_owned());
            json!({ "id": board, "name": name, "chip": chip })
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalOpenParams {
    path: PathBuf,
}

async fn local_open(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: LocalOpenParams = parse_params(params)?;
    let project = state.local.add(&p.path).map_err(build_error)?;
    Ok(json!({ "project": project }))
}

async fn local_list(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let _: NoParams = parse_params(params)?;
    Ok(json!({ "projects": state.local.list(), "idf": state.idf() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalIdParams {
    id: String,
}

async fn local_get(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: LocalIdParams = parse_params(params)?;
    let project = state
        .local
        .get(&p.id)
        .ok_or_else(|| build_error(esp32bench_build::Error::NoLocalProject(p.id.clone())))?;
    let exists = project.path.join("CMakeLists.txt").is_file();
    Ok(json!({
        "project": project,
        "boards": local_boards(&state).await,
        "idf": state.idf(),
        "exists": exists,
    }))
}

async fn local_remove(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: LocalIdParams = parse_params(params)?;
    let removed = state.local.remove(&p.id).map_err(build_error)?;
    Ok(json!({ "removed": removed }))
}

fn toolchain_env(state: &State) -> esp32bench_toolchain::Env {
    esp32bench_toolchain::Env {
        arduino_cli: state.arduino_cli.clone(),
        arduino_data: state.arduino_data.clone(),
        sketchbook: state.sketchbook.clone(),
        distro: esp32bench_distro::Distro::detect(),
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
    // Where the distribution packages arduino-cli, installing it needs root.
    let distro = esp32bench_distro::Distro::detect();
    if id == esp32bench_toolchain::Id::ArduinoCli
        && let Some(cmd) = distro.install_hint(&[esp32bench_distro::Need::ArduinoCli])
    {
        return Err(RpcError::new(
            code::FAILED,
            format!(
                "arduino-cli is a system package on {}; install it with: {cmd}",
                distro.name()
            ),
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

/// Commands for a running simulator session, sent to the task that owns it.
enum SimCmd {
    Write(Vec<u8>),
    SetPin(u8, bool, tokio::sync::oneshot::Sender<Result<(), String>>),
    Control(
        String,
        String,
        Value,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    Reset(tokio::sync::oneshot::Sender<Result<(), String>>),
    Debug(
        DebugReq,
        tokio::sync::oneshot::Sender<Result<Value, String>>,
    ),
}

/// GPIO numbers a project's build for `board` uses: the numeric entries of its `pins`
/// (GND, 3V3 and the like are not GPIOs), sorted, at most GPIO 48.
fn project_pins(manifest: &esp32bench_library::Manifest, project: &str, board: &str) -> Vec<u8> {
    let mut pins: Vec<u8> = manifest
        .projects
        .iter()
        .filter(|p| p.id == project)
        .flat_map(|p| &p.builds)
        .filter(|b| b.board == board)
        .flat_map(|b| &b.pins)
        .filter_map(|s| s.trim().parse::<u8>().ok())
        .filter(|&g| g <= 48)
        .collect();
    pins.sort_unstable();
    pins.dedup();
    pins
}

/// How often pin changes go out at most: firmware can toggle a pin far faster than a
/// person can see, so changes are batched and each pin's latest state is sent.
const PIN_BATCH: std::time::Duration = std::time::Duration::from_millis(50);

/// A request for the GDB attached to a simulator started with `debug: true`.
enum DebugReq {
    Break(String),
    Delete(u32),
    Control(String),
    Inspect,
}

/// GDB attached to a paused simulator, ready for requests.
struct Debugger {
    gdb: esp32bench_debug::Gdb,
    events: tokio::sync::mpsc::UnboundedReceiver<esp32bench_debug::Event>,
}

async fn start_debugger(
    state: &State,
    chip: &str,
    elf: &Path,
    port: u16,
) -> Result<Debugger, RpcError> {
    let gdb_path = esp32bench_debug::find_gdb(&state.arduino_data, chip).ok_or_else(|| {
        RpcError::new(
            code::UNAVAILABLE,
            "GDB comes with the ESP32 core; install it from the toolchain setup",
        )
    })?;
    let (mut gdb, events) = esp32bench_debug::Gdb::start(&gdb_path, elf)
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    gdb.connect(port)
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    Ok(Debugger { gdb, events })
}

async fn debug_request(gdb: &mut esp32bench_debug::Gdb, req: DebugReq) -> Result<Value, String> {
    let r = match req {
        DebugReq::Break(loc) => gdb.break_insert(&loc).await.map(|b| {
            let mut bp = stop_frame(&b);
            bp["number"] = json!(b["number"].as_str().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0));
            json!({ "breakpoint": bp })
        }),
        DebugReq::Delete(n) => gdb.break_delete(n).await.map(|()| json!({})),
        DebugReq::Control(a) => gdb.control(&a).await.map(|()| json!({})),
        DebugReq::Inspect => async {
            let frames = gdb.frames().await?;
            let registers = gdb.registers().await?;
            Ok(json!({
                "frames": frames.as_array().into_iter().flatten().map(stop_frame).collect::<Vec<_>>(),
                "registers": registers.into_iter().map(|(name, value)| json!({ "name": name, "value": value })).collect::<Vec<_>>(),
            }))
        }
        .await,
    };
    r.map_err(|e| e.to_string())
}

/// A GDB frame as the API reports it: function, source file and line when known.
fn stop_frame(f: &Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("addr".into(), json!(f["addr"].as_str().unwrap_or("")));
    if let Some(func) = f["func"].as_str().filter(|s| *s != "??") {
        out.insert("func".into(), json!(func));
    }
    if let Some(file) = f["fullname"].as_str().or(f["file"].as_str()) {
        out.insert("file".into(), json!(file));
    }
    if let Some(line) = f["line"].as_str().and_then(|l| l.parse::<u32>().ok()) {
        out.insert("line".into(), json!(line));
    }
    Value::Object(out)
}

/// Removes a simulator from the table when its subscription ends; dropping the task
/// also drops its Session, which stops QEMU.
struct SimEntry {
    state: Arc<State>,
    key: (u64, u64),
}

impl Drop for SimEntry {
    fn drop(&mut self) {
        let mut sims = self.state.sims.lock().unwrap_or_else(|p| p.into_inner());
        sims.remove(&self.key);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SimStartParams {
    project: String,
    board: String,
    #[serde(default)]
    debug: bool,
    /// The simulated Wi-Fi network, where the machine has a radio for it.
    #[serde(default = "yes")]
    network: bool,
    /// Join a library sketch to that network through its settings block.
    #[serde(default = "yes", rename = "joinWifi")]
    join_wifi: bool,
    /// Count time in instructions (QEMU's -icount): chip-like timing, independent of
    /// this computer's load.
    #[serde(default)]
    icount: bool,
}

async fn sim_start(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SimStartParams = parse_params(params)?;
    for id in [&p.project, &p.board] {
        esp32bench_build::safe_id(id).map_err(|e| RpcError::invalid_params(e.to_string()))?;
    }
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
    let chip_id = manifest
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
    // Only chips QEMU covers; never claim a simulation we cannot run (rule 6).
    let chip: esp32bench_sim::Chip = chip_id.parse().map_err(|_| {
        RpcError::new(
            code::FAILED,
            format!("{chip_id} is workbench-only: the simulator runs ESP32, ESP32-S3 and ESP32-C3"),
        )
    })?;
    let image = state
        .builds_root
        .join(&p.project)
        .join(&p.board)
        .join("out")
        .join(format!("{}.ino.merged.bin", p.project));
    if !image.is_file() {
        return Err(RpcError::new(
            code::NOT_FOUND,
            format!("`{}` has not been built for `{}` yet", p.project, p.board),
        ));
    }
    let elf = image.with_file_name(format!("{}.ino.elf", p.project));
    if p.debug && !elf.is_file() {
        return Err(RpcError::new(
            code::NOT_FOUND,
            format!("the build of `{}` has no ELF file to debug", p.project),
        ));
    }
    // lcgamboa's fork when it is built, else Espressif's QEMU.
    let launch = esp32bench_sim::engine::launch(chip, None)
        .map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))?;
    let engine = launch.engine;
    // QEMU's GDB stub listens on loopback only; any free port will do.
    let gdb_port = if p.debug {
        let l = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| RpcError::internal(e.to_string()))?;
        Some(
            l.local_addr()
                .map_err(|e| RpcError::internal(e.to_string()))?
                .port(),
        )
    } else {
        None
    };
    // The GPIOs the project uses, from the library (never from the caller), bridged
    // when the engine has the fork's hooks (lcgamboa's library).
    // And the pins of the parts placed on this board in the app.
    let circuit = esp32bench_circuit::load(&state.circuits_root, &p.project, &p.board)
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    let pins: Vec<u8> = if launch.hooks {
        let mut pins = project_pins(&manifest, &p.project, &p.board);
        pins.extend(circuit.pins());
        pins.sort_unstable();
        pins.dedup();
        pins
    } else {
        Vec::new()
    };
    // Whether the circuit's parts work in this simulation: the host runs them.
    let launch_parts = launch.hooks && !circuit.parts.is_empty();
    let mut config = launch.config(chip, image);
    config.bridge = pins.clone();
    // Boards whose Serial is the chip's USB (the C3 SuperMini) need the USB console.
    config.usb_console = launch.usb_console && esp32bench_build::serial_on_usb(&p.board);
    // The simulated Wi-Fi network: restricted, nothing outside reachable; the
    // sketch's port 80 forwarded to a free port on loopback.
    if launch.network && p.network {
        config.network = Some(
            esp32bench_sim::network::Network::on_a_free_port()
                .map_err(|e| RpcError::internal(e.to_string()))?,
        );
    }
    let network = config.network;
    config.icount = p.icount;
    let mut joiner = (network.is_some() && p.join_wifi).then(esp32bench_sim::network::Joiner::new);
    if launch_parts {
        config.circuit =
            Some(serde_json::to_string(&circuit).map_err(|e| RpcError::internal(e.to_string()))?);
    }
    let bridged_pins = pins.clone();
    // Where to remember slider values moved during the run.
    let circuit_at = (
        state.circuits_root.clone(),
        p.project.clone(),
        p.board.clone(),
    );
    config.gdb_port = gdb_port;
    // Debugging starts paused at the first instruction, for breakpoints in setup.
    config.start_paused = p.debug;
    let mut session = esp32bench_sim::Session::start(config)
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    let mut debugger = match gdb_port {
        Some(port) => Some(start_debugger(&state, &chip_id, &elf, port).await?),
        None => None,
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SimCmd>();
    let conn = ctx.connection_id();
    let id = ctx.subscribe(move |sub| {
        let key = (conn, sub.id());
        state
            .sims
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, tx);
        let trace: SharedTrace = Arc::default();
        {
            let mut traces = state.traces.lock().unwrap_or_else(|p| p.into_inner());
            if traces.len() == TRACES_KEPT {
                traces.pop_front();
            }
            traces.push_back((key, trace.clone()));
        }
        let entry = SimEntry {
            state: state.clone(),
            key,
        };
        async move {
            let _entry = entry;
            use base64::Engine;
            // The virtual RGB panel: polling it is what lets firmware's draws finish,
            // so it runs for the whole session; frames go out only once it is in use.
            let mut display = Some(esp32bench_sim::display::Display::new(&session.paths().dir.clone()));
            let mut gpio = session.gpio.take();
            let bridged = bridged_pins;
            // Part changes since the last batch: id -> its sim.parts item.
            let mut parts_changed: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            // gpio -> (level, output), and the pins changed since the last batch.
            let mut pin_state: std::collections::BTreeMap<u8, (Option<bool>, Option<bool>)> =
                std::collections::BTreeMap::new();
            let mut pin_changed: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
            // Edges since the last sim.trace event, and how many did not fit in it.
            let mut trace_batch: Vec<Value> = Vec::new();
            let mut trace_skipped: u64 = 0;
            let mut pin_flush = tokio::time::Instant::now();
            loop {
                let wait = display.as_ref().map(esp32bench_sim::display::Display::interval);
                tokio::select! {
                    () = async {
                        match wait {
                            Some(w) => tokio::time::sleep(w).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        let Some(d) = display.as_mut() else { continue };
                        match d.poll(&mut session.qmp).await {
                            Ok(Some(frame)) => {
                                sub.send("sim.frame", json!({
                                    "width": frame.width,
                                    "height": frame.height,
                                    "format": "rgb888",
                                    "data": base64::engine::general_purpose::STANDARD.encode(&frame.rgb),
                                }));
                            }
                            Ok(None) => {}
                            // A QEMU without screendump: stop asking.
                            Err(_) => display = None,
                        }
                    }
                    ev = async {
                        match gpio.as_mut() {
                            Some(g) => g.next_event().await,
                            None => std::future::pending().await,
                        }
                    } => match ev {
                        Ok(Some(ev)) => {
                            use esp32bench_sim::gpio::PinEvent;
                            let edge = trace.lock().unwrap_or_else(|p| p.into_inner()).record(&ev);
                            if let Some(e) = edge {
                                if trace_batch.is_empty() && trace_skipped == 0
                                    && pin_changed.is_empty() && parts_changed.is_empty()
                                {
                                    pin_flush = tokio::time::Instant::now() + PIN_BATCH;
                                }
                                if trace_batch.len() < TRACE_BATCH_MAX {
                                    let mut item = json!({ "t": e.time_ns, "gpio": e.gpio, "level": e.level });
                                    if e.input {
                                        item["input"] = json!(true);
                                    }
                                    trace_batch.push(item);
                                } else {
                                    trace_skipped += 1;
                                }
                            }
                            let gpio_num = match ev {
                                // Inputs the host drove are only for the trace.
                                PinEvent::Input { .. } => continue,
                                PinEvent::Level { gpio, level, .. } => {
                                    pin_state.entry(gpio).or_default().0 = Some(level);
                                    gpio
                                }
                                PinEvent::Direction { gpio, output } => {
                                    pin_state.entry(gpio).or_default().1 = Some(output);
                                    gpio
                                }
                                PinEvent::Part { id, state } => {
                                    let mut item = json!({ "id": id, "state": state });
                                    if let Some(on) = item["state"]["on"].as_bool() {
                                        item["on"] = json!(on);
                                    }
                                    if parts_changed.is_empty() && pin_changed.is_empty() {
                                        pin_flush = tokio::time::Instant::now() + PIN_BATCH;
                                    }
                                    parts_changed.insert(id, item);
                                    continue;
                                }
                                PinEvent::PartError { id, message } => {
                                    if parts_changed.is_empty() && pin_changed.is_empty() {
                                        pin_flush = tokio::time::Instant::now();
                                    }
                                    parts_changed.insert(id.clone(), json!({ "id": id, "error": message }));
                                    continue;
                                }
                                // The host already limits these to 20 a second.
                                PinEvent::Picture { id, width, height, pixels } => {
                                    sub.send("sim.partPicture", json!({
                                        "id": id,
                                        "width": width,
                                        "height": height,
                                        "data": base64::engine::general_purpose::STANDARD.encode(&pixels),
                                    }));
                                    continue;
                                }
                            };
                            if pin_changed.is_empty() {
                                pin_flush = tokio::time::Instant::now() + PIN_BATCH;
                            }
                            pin_changed.insert(gpio_num);
                        }
                        // The host closed the link: no more pin events this session.
                        Ok(None) | Err(_) => gpio = None,
                    },
                    () = tokio::time::sleep_until(pin_flush), if !pin_changed.is_empty() || !parts_changed.is_empty()
                        || !trace_batch.is_empty() || trace_skipped > 0 => {
                        if !trace_batch.is_empty() || trace_skipped > 0 {
                            let mut params = json!({ "edges": std::mem::take(&mut trace_batch) });
                            if trace_skipped > 0 {
                                params["skipped"] = json!(std::mem::take(&mut trace_skipped));
                            }
                            sub.send("sim.trace", params);
                        }
                        if !parts_changed.is_empty() {
                            let changed: Vec<Value> =
                                std::mem::take(&mut parts_changed).into_values().collect();
                            sub.send("sim.parts", json!({ "parts": changed }));
                        }
                        if pin_changed.is_empty() {
                            continue;
                        }
                        let pins: Vec<Value> = std::mem::take(&mut pin_changed)
                            .into_iter()
                            .map(|g| {
                                let (level, output) = pin_state.get(&g).copied().unwrap_or_default();
                                let mut o = json!({ "gpio": g });
                                if let Some(l) = level {
                                    o["level"] = json!(l);
                                }
                                if let Some(out) = output {
                                    o["output"] = json!(out);
                                }
                                o
                            })
                            .collect();
                        sub.send("sim.pins", json!({ "pins": pins }));
                    }
                    chunk = session.uart.read_chunk() => match chunk {
                        Ok(Some(bytes)) => {
                            let sent = sub.send("sim.output", json!({
                                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                                "text": String::from_utf8_lossy(&bytes),
                            }));
                            if !sent {
                                break;
                            }
                            // A library sketch asking for its Wi-Fi settings: give it
                            // the simulated network's (sim.wifiJoined says so).
                            if let Some(lines) = joiner.as_mut().and_then(|j| j.feed(&bytes)) {
                                let _ = session.uart.write(&lines).await;
                                sub.send("sim.wifiJoined", json!({
                                    "ssid": esp32bench_sim::network::SSID,
                                }));
                            }
                        }
                        Ok(None) => {
                            sub.send("sim.exited", json!({ "reason": "QEMU closed the UART" }));
                            break;
                        }
                        Err(e) => {
                            sub.send("sim.exited", json!({ "reason": e.to_string() }));
                            break;
                        }
                    },
                    cmd = rx.recv() => match cmd {
                        Some(SimCmd::Write(bytes)) => {
                            let _ = session.uart.write(&bytes).await;
                        }
                        Some(SimCmd::SetPin(g, level, done)) => {
                            let r = match gpio.as_mut().filter(|_| bridged.contains(&g)) {
                                Some(link) => link.set(g, level).await.map_err(|e| e.to_string()),
                                None => Err("this simulator has no pin access".into()),
                            };
                            let _ = done.send(r);
                        }
                        Some(SimCmd::Control(id, name, value, done)) => {
                            let r = match gpio.as_mut().filter(|_| launch_parts) {
                                Some(link) => link.control(&id, &name, &value).await.map_err(|e| e.to_string()),
                                None => Err("this simulation runs no parts".into()),
                            };
                            if let (Ok(()), Some(v)) = (&r, value.as_f64()) {
                                let (root, project, board) = circuit_at.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    remember_control(&root, &project, &board, &id, &name, v)
                                })
                                .await;
                            }
                            let _ = done.send(r);
                        }
                        Some(SimCmd::Reset(done)) => {
                            let _ = done.send(session.reset().await.map_err(|e| e.to_string()));
                        }
                        Some(SimCmd::Debug(req, done)) => {
                            let r = match debugger.as_mut() {
                                Some(d) => debug_request(&mut d.gdb, req).await,
                                None => Err("this simulator was not started for debugging".into()),
                            };
                            let _ = done.send(r);
                        }
                        None => break,
                    },
                    ev = async {
                        match debugger.as_mut() {
                            Some(d) => d.events.recv().await,
                            None => std::future::pending().await,
                        }
                    } => match ev {
                        Some(esp32bench_debug::Event::Stopped(v)) => {
                            let mut params = json!({
                                "reason": v["reason"].as_str().unwrap_or("paused"),
                                "frame": stop_frame(&v["frame"]),
                            });
                            if let Some(n) = v["bkptno"].as_str().and_then(|n| n.parse::<u32>().ok()) {
                                params["breakpoint"] = json!(n);
                            }
                            sub.send("debug.stopped", params);
                        }
                        Some(esp32bench_debug::Event::Running) => {
                            sub.send("debug.running", json!({}));
                        }
                        Some(esp32bench_debug::Event::Console(_)) => {}
                        Some(esp32bench_debug::Event::Exited) | None => {
                            debugger = None;
                            sub.send("debug.exited", json!({}));
                        }
                    },
                }
            }
            // Ask QEMU to quit; the Session's drop kills it if it does not.
            let _ = session.shutdown().await;
        }
    });
    Ok(json!({
        "subscription": id,
        "chip": chip,
        "debug": p.debug,
        "engine": engine,
        "pins": pins,
        "partsActive": launch_parts,
        "icount": p.icount,
        "network": network.map(|n| json!({
            "ssid": esp32bench_sim::network::SSID,
            "webUrl": n.web_url(),
        })),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceSaveParams {
    subscription: u64,
    path: PathBuf,
}

/// Write a simulation's signal trace (running or ended) to `path` as VCD.
async fn sim_trace_save(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: TraceSaveParams = parse_params(params)?;
    if !p.path.is_absolute() {
        return Err(RpcError::invalid_params("path must be absolute"));
    }
    let key = (ctx.connection_id(), p.subscription);
    let trace = state
        .traces
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, t)| t.clone())
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                format!("no recent simulator {} on this connection", p.subscription),
            )
        })?;
    let (vcd, edges, dropped) = {
        let t = trace.lock().unwrap_or_else(|p| p.into_inner());
        (t.to_vcd(env!("CARGO_PKG_VERSION")), t.len(), t.dropped())
    };
    tokio::fs::write(&p.path, vcd)
        .await
        .map_err(|e| RpcError::new(code::FAILED, format!("writing {}: {e}", p.path.display())))?;
    Ok(json!({ "path": p.path, "edges": edges, "dropped": dropped }))
}

fn sim_sender(
    state: &State,
    ctx: &Ctx,
    subscription: u64,
) -> Result<tokio::sync::mpsc::UnboundedSender<SimCmd>, RpcError> {
    state
        .sims
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(ctx.connection_id(), subscription))
        .cloned()
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                format!("no simulator {subscription} on this connection"),
            )
        })
}

async fn sim_write(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    use base64::Engine;
    let p: SerialWriteParams = parse_params(params)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.data)
        .map_err(|e| RpcError::invalid_params(format!("data is not base64: {e}")))?;
    sim_sender(&state, &ctx, p.subscription)?
        .send(SimCmd::Write(bytes))
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DebugBreakParams {
    subscription: u64,
    location: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DebugDeleteParams {
    subscription: u64,
    breakpoint: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DebugControlParams {
    subscription: u64,
    action: String,
}

async fn debug_call(
    state: &State,
    ctx: &Ctx,
    subscription: u64,
    req: DebugReq,
) -> Result<Value, RpcError> {
    let (done, result) = tokio::sync::oneshot::channel();
    sim_sender(state, ctx, subscription)?
        .send(SimCmd::Debug(req, done))
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?;
    result
        .await
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?
        .map_err(|e| RpcError::new(code::FAILED, e))
}

async fn debug_break(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: DebugBreakParams = parse_params(params)?;
    if !esp32bench_debug::valid_location(&p.location) {
        return Err(RpcError::invalid_params(
            "location must be a function name or file:line",
        ));
    }
    debug_call(&state, &ctx, p.subscription, DebugReq::Break(p.location)).await
}

async fn debug_delete(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: DebugDeleteParams = parse_params(params)?;
    debug_call(&state, &ctx, p.subscription, DebugReq::Delete(p.breakpoint)).await
}

async fn debug_control(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: DebugControlParams = parse_params(params)?;
    if !["continue", "next", "step", "finish", "interrupt"].contains(&p.action.as_str()) {
        return Err(RpcError::invalid_params(
            "action must be continue, next, step, finish or interrupt",
        ));
    }
    debug_call(&state, &ctx, p.subscription, DebugReq::Control(p.action)).await
}

async fn debug_inspect(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialRefParams = parse_params(params)?;
    debug_call(&state, &ctx, p.subscription, DebugReq::Inspect).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CircuitRef {
    project: String,
    board: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CircuitSetParams {
    project: String,
    board: String,
    circuit: esp32bench_circuit::Circuit,
}

fn check_ids(project: &str, board: &str) -> Result<(), RpcError> {
    for id in [project, board] {
        esp32bench_build::safe_id(id).map_err(|e| RpcError::invalid_params(e.to_string()))?;
    }
    Ok(())
}

/// The kinds of part the app can place (schema/part-model), without their scripts.
fn circuit_models() -> Result<Value, RpcError> {
    let models: Vec<esp32bench_circuit::Model> = esp32bench_circuit::models()
        .into_iter()
        .map(|m| m.model)
        .collect();
    Ok(json!({ "models": models }))
}

/// Keep a slider's value in the saved circuit (circuit 1.2 `controls`), so the part
/// starts there next run. Only for a part that is still placed and a value its model
/// accepts; anything else (the circuit changed meanwhile) is left alone.
fn remember_control(root: &Path, project: &str, board: &str, part: &str, control: &str, v: f64) {
    let Ok(mut c) = esp32bench_circuit::load(root, project, board) else {
        return;
    };
    let Some(p) = c.parts.iter_mut().find(|p| p.id == part) else {
        return;
    };
    p.controls.insert(control.to_owned(), v);
    // A value the model refuses is not saved; the run itself already has it.
    let _ = esp32bench_circuit::save(root, project, board, &c);
}

async fn circuit_get(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: CircuitRef = parse_params(params)?;
    check_ids(&p.project, &p.board)?;
    let c = esp32bench_circuit::load(&state.circuits_root, &p.project, &p.board)
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    Ok(json!({ "circuit": c }))
}

async fn circuit_set(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: CircuitSetParams = parse_params(params)?;
    check_ids(&p.project, &p.board)?;
    p.circuit
        .validate()
        .map_err(|e| RpcError::invalid_params(e.to_string()))?;
    esp32bench_circuit::save(&state.circuits_root, &p.project, &p.board, &p.circuit)
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlParams {
    subscription: u64,
    part: String,
    control: String,
    value: Value,
}

/// Work a part's control. The part's model says which controls it has and what
/// values they take; the host checks, and a bad control comes back in sim.parts as the
/// part's `error`.
async fn sim_control(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: ControlParams = parse_params(params)?;
    if !(p.value.is_boolean() || p.value.is_number()) {
        return Err(RpcError::invalid_params(
            "value must be true, false or a number",
        ));
    }
    for name in [&p.part, &p.control] {
        if name.is_empty() || name.len() > 32 || name.contains(char::is_whitespace) {
            return Err(RpcError::invalid_params(
                "part and control are short names without spaces",
            ));
        }
    }
    let (done, result) = tokio::sync::oneshot::channel();
    sim_sender(&state, &ctx, p.subscription)?
        .send(SimCmd::Control(p.part, p.control, p.value, done))
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?;
    result
        .await
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?
        .map_err(|e| RpcError::new(code::FAILED, e))?;
    Ok(json!({}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetPinParams {
    subscription: u64,
    gpio: u8,
    level: bool,
}

async fn sim_set_pin(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SetPinParams = parse_params(params)?;
    let (done, result) = tokio::sync::oneshot::channel();
    sim_sender(&state, &ctx, p.subscription)?
        .send(SimCmd::SetPin(p.gpio, p.level, done))
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?;
    result
        .await
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?
        .map_err(|_| {
            RpcError::new(
                code::FAILED,
                format!("GPIO {} is not one of this simulation's pins", p.gpio),
            )
        })?;
    Ok(json!({}))
}

async fn sim_reset(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialRefParams = parse_params(params)?;
    let (done, result) = tokio::sync::oneshot::channel();
    sim_sender(&state, &ctx, p.subscription)?
        .send(SimCmd::Reset(done))
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?;
    result
        .await
        .map_err(|_| RpcError::new(code::NOT_FOUND, "the simulator has stopped"))?
        .map_err(|e| RpcError::new(code::FAILED, e))?;
    Ok(json!({}))
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
