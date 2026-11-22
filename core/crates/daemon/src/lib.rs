//! The core daemon's method table and socket setup. `main.rs` only parses arguments
//! and handles signals; everything here is testable without a running daemon.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use esp32bench_rpc::Listener;
use esp32bench_rpc::{Ctx, Router, RpcError, code, parse_params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Protocol version, as in `schema/rpc/index.json`. A test keeps them equal.
pub const PROTOCOL: &str = "0.33.0";
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
    /// Each open port's recording to a file (serial.record), if one is running.
    serial_records: std::sync::Mutex<HashMap<(u64, u64), SharedRecording>>,
    /// Ports serial.open may open instead of those device.list reports (tests).
    pub serial_allow: Option<Vec<PathBuf>>,
    /// How long a port opened with `reconnect` is waited for after it goes away.
    pub serial_reconnect_for: std::time::Duration,
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
    /// Library projects the user starred (library.star).
    pub starred: esp32bench_library::starred::Starred,
    /// Chips found behind USB serial adapters by board.inspect.
    pub known_boards: esp32bench_device::Known,
    /// Lines sent often, per project (commands.get/commands.set).
    pub commands: esp32bench_library::commands::Commands,
    /// Breakpoints kept per project (breakpoints.get/breakpoints.set), in the same
    /// format: an object from project id to its locations.
    pub breakpoints: esp32bench_library::commands::Commands,
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
type SharedRecording = Arc<std::sync::Mutex<Option<esp32bench_serial::record::Recording>>>;
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
        drop(ports);
        self.state
            .serial_records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
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
            serial_records: std::sync::Mutex::new(HashMap::new()),
            serial_allow: None,
            serial_reconnect_for: std::time::Duration::from_secs(30),
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
            starred: esp32bench_library::starred::Starred::new(
                esp32bench_sim::install::data_dir()
                    .map(|d| d.join("starred.json"))
                    .unwrap_or_else(|_| PathBuf::from("esp32bench-starred.json")),
            ),
            known_boards: esp32bench_device::Known::new(
                esp32bench_sim::install::data_dir()
                    .map(|d| d.join("boards.json"))
                    .unwrap_or_else(|_| PathBuf::from("esp32bench-boards.json")),
            ),
            commands: esp32bench_library::commands::Commands::new(
                esp32bench_sim::install::data_dir()
                    .map(|d| d.join("commands.json"))
                    .unwrap_or_else(|_| PathBuf::from("esp32bench-commands.json")),
            ),
            breakpoints: esp32bench_library::commands::Commands::new(
                esp32bench_sim::install::data_dir()
                    .map(|d| d.join("breakpoints.json"))
                    .unwrap_or_else(|_| PathBuf::from("esp32bench-breakpoints.json")),
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
    let stars = state.clone();
    let devices = state.clone();
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
    let (c1, c2, c3, c4, c5) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state,
    );
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
        .method("console.save", move |p, _ctx| async move {
            console_save(p).await
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
        .method("circuit.suggest", move |p, _ctx| {
            let s = c5.clone();
            async move { circuit_suggest(p, s).await }
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
        .method("serial.record", {
            let s = stars.clone();
            move |p, ctx| {
                let s = s.clone();
                async move { serial_record(p, ctx, s).await }
            }
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
        .method("build.status", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { build_status(p, s).await }
            }
        })
        .method("commands.get", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { commands(p, s, false).await }
            }
        })
        .method("commands.set", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { commands(p, s, true).await }
            }
        })
        .method("breakpoints.get", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { breakpoints(p, s, false).await }
            }
        })
        .method("breakpoints.set", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { breakpoints(p, s, true).await }
            }
        })
        .method("library.export", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { library_export(p, s).await }
            }
        })
        .method("plot.save", |p, _| async move { plot_save(p).await })
        .method("frame.save", |p, _| async move { frame_save(p).await })
        .method("library.star", {
            let s = stars.clone();
            move |p, _| {
                let s = s.clone();
                async move { library_star(p, s).await }
            }
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
        .method("device.list", {
            let s = devices.clone();
            move |p, _| {
                let s = s.clone();
                async move { device_list(p, s).await }
            }
        })
        .method("device.watch", {
            let s = devices.clone();
            move |p, ctx| {
                let s = s.clone();
                async move { device_watch(p, ctx, s).await }
            }
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
    let starred = state.starred.list();
    let projects: Vec<Value> = m
        .summaries()
        .into_iter()
        .map(|p| {
            let mut v = json!(p);
            v["starred"] = json!(starred.contains(&p.id));
            v["built"] = json!(esp32bench_build::built_boards(&state.builds_root, &p.id));
            v
        })
        .collect();
    let mut out = json!({
        "available": true,
        "source": state.library_source,
        "version": m.version,
        "generated": m.generated,
        "projects": projects,
    });
    if let Some(t) = esp32bench_library::modified_secs(&state.library.path()) {
        out["fetchedAt"] = json!(t);
    }
    if let Some(d) = m.newer_desktop(env!("CARGO_PKG_VERSION")) {
        out["update"] = json!(d);
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StarParams {
    id: String,
    starred: bool,
}

async fn library_star(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: StarParams = parse_params(params)?;
    if p.id.is_empty() || p.id.len() > 200 {
        return Err(RpcError::new(code::INVALID_PARAMS, "bad project id"));
    }
    let starred = state
        .starred
        .set(&p.id, p.starred)
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    Ok(json!({ "starred": starred }))
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
    // What each pin is wired to, per board, for naming pins in the Pins and Trace docks.
    let labels: serde_json::Map<String, Value> = project
        .builds
        .iter()
        .map(|b| {
            let wiring: Vec<esp32bench_circuit::suggest::Wire> = b
                .wiring
                .iter()
                .map(|w| esp32bench_circuit::suggest::Wire {
                    from: &w.from,
                    to: &w.to,
                })
                .collect();
            (
                b.board.clone(),
                json!(esp32bench_circuit::suggest::pin_labels(&wiring)),
            )
        })
        .collect();
    Ok(json!({ "project": project, "boards": boards, "pinLabels": labels }))
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
    // Prebuilt firmware is downloaded, not built: no toolchain, but the same consent.
    if let Some(image) = &plan.image {
        let source = esp32bench_library::image_source(&state.library_source, &image.url)
            .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
        return prebuilt_start(p.consent.as_deref(), plan, source, ctx, state);
    }
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
    let mut explain = esp32bench_library::explain::Explainer::new(&manifest.explain);
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.build_lock.lock().await;
        let dirs = esp32bench_build::Dirs::new(&state.builds_root, &plan);
        let mut first = esp32bench_build::diagnostics::Finder::new(None);
        let mut size = esp32bench_build::diagnostics::SizeReader::default();
        let r = esp32bench_build::run(&plan, &code, &tools, &dirs, |stream, line| {
            first.line(&line);
            size.line(&line);
            let mut params = json!({ "stream": stream, "line": line });
            if let Some(level) = esp32bench_build::diagnostics::line_level(&line) {
                params["level"] = json!(level);
                if let Some(at) = first.location(&line) {
                    params["location"] = json!(at);
                }
            }
            let found = explain.line_findings(&line);
            if !found.is_empty() {
                params["explain"] = json!(found);
            }
            sub.send("build.log", params);
        })
        .await;
        if r.is_ok() {
            esp32bench_build::write_stamp(&dirs.out, &plan);
        }
        let mut event = finished_event(r, first.first());
        if event["ok"] == true
            && let Some(sz) = size.size()
        {
            event["size"] = json!(sz);
        }
        sub.send("build.finished", event);
    });
    Ok(json!({ "job": job, "plan": result_plan }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportParams {
    id: String,
    board: String,
    dir: PathBuf,
    #[serde(default)]
    reuse: bool,
}

/// Save a library project's sketch for one board into a folder the user chose, for
/// the Arduino IDE. Nothing is built, so no consent is asked.
async fn library_export(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: ExportParams = parse_params(params)?;
    if !p.dir.is_absolute() {
        return Err(RpcError::invalid_params("dir must be an absolute path"));
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
    let (plan, code) = esp32bench_build::plan(&manifest, &p.id, &p.board).map_err(build_error)?;
    let (ino, existing) = esp32bench_build::export_sketch_or_reuse(&plan, &code, &p.dir, p.reuse)
        .map_err(build_error)?;
    Ok(json!({ "path": ino, "libraries": plan.install, "existing": existing }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandsParams {
    #[serde(default)]
    project: String,
    #[serde(default)]
    commands: Option<Vec<String>>,
}

/// commands.get and commands.set: the lines a project's board is often sent.
async fn commands(params: Value, state: Arc<State>, set: bool) -> Result<Value, RpcError> {
    let p: CommandsParams = parse_params(params)?;
    if p.project.len() > 200 {
        return Err(RpcError::invalid_params("bad project id"));
    }
    let lines = if set {
        let lines = p
            .commands
            .ok_or_else(|| RpcError::invalid_params("commands.set needs commands"))?;
        state
            .commands
            .set(&p.project, lines)
            .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
    } else {
        if p.commands.is_some() {
            return Err(RpcError::invalid_params("commands.get takes no commands"));
        }
        state.commands.get(&p.project)
    };
    Ok(json!({ "commands": lines }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BreakpointsParams {
    project: String,
    #[serde(default)]
    breakpoints: Option<Vec<String>>,
}

/// breakpoints.get and breakpoints.set: where a project's debugging stops, set again
/// at the next debug run. Each location is one debug.break takes.
async fn breakpoints(params: Value, state: Arc<State>, set: bool) -> Result<Value, RpcError> {
    let p: BreakpointsParams = parse_params(params)?;
    if p.project.is_empty() || p.project.len() > 200 {
        return Err(RpcError::invalid_params("bad project id"));
    }
    let kept = if set {
        let list = p
            .breakpoints
            .ok_or_else(|| RpcError::invalid_params("breakpoints.set needs breakpoints"))?;
        if let Some(bad) = list.iter().find(|l| !esp32bench_debug::valid_location(l)) {
            return Err(RpcError::invalid_params(format!(
                "`{bad}` is not a function name or file:line"
            )));
        }
        state
            .breakpoints
            .set(&p.project, list)
            .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?
    } else {
        if p.breakpoints.is_some() {
            return Err(RpcError::invalid_params(
                "breakpoints.get takes no breakpoints",
            ));
        }
        state.breakpoints.get(&p.project)
    };
    Ok(json!({ "breakpoints": kept }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildStatusParams {
    project: String,
    board: String,
}

/// Whether `project` is built for `board`, when, and (for a library project) whether
/// that build is what the library asks for now.
async fn build_status(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: BuildStatusParams = parse_params(params)?;
    for id in [&p.project, &p.board] {
        esp32bench_build::safe_id(id).map_err(build_error)?;
    }
    let out = state
        .builds_root
        .join(&p.project)
        .join(&p.board)
        .join("out");
    let image = out.join(format!("{}.ino.merged.bin", p.project));
    let Some(stamp) = esp32bench_build::read_stamp(&out, &image) else {
        return Ok(json!({ "built": false }));
    };
    let mut r = json!({ "built": true, "builtAt": stamp.built_at });
    // A local project's files can change at any time, so its build is never called
    // current; a library build is current while the plan is the same.
    if !esp32bench_build::idf::is_local(&p.project) {
        let current = match state.library.load().await {
            Ok(Some(m)) => esp32bench_build::plan(&m, &p.project, &p.board)
                .map(|(plan, _)| esp32bench_build::plan_id(&plan) == stamp.plan)
                .unwrap_or(false),
            _ => false,
        };
        r["current"] = json!(current);
    }
    Ok(r)
}

/// build.finished's params for a build's result, with the first compiler error when
/// it failed and one was printed.
fn finished_event<E: std::fmt::Display>(
    r: Result<esp32bench_build::Output, E>,
    first: Option<&esp32bench_build::diagnostics::FirstError>,
) -> Value {
    match r {
        Ok(out) => json!({ "ok": true, "image": out.image, "elf": out.elf }),
        Err(e) => {
            let mut v = json!({ "ok": false, "error": e.to_string() });
            if let Some(f) = first {
                v["firstError"] = json!(f);
            }
            v
        }
    }
}

/// A library project's prebuilt firmware: after consent for exactly this image, a job
/// that downloads it, checks it against the library and keeps it where a build's image
/// goes. The events are build.start's.
fn prebuilt_start(
    consent: Option<&str>,
    plan: esp32bench_build::Plan,
    source: String,
    ctx: Ctx,
    state: Arc<State>,
) -> Result<Value, RpcError> {
    let granted = consent.is_some_and(|t| state.consent.redeem(t, &plan));
    if !granted {
        let token = state
            .consent
            .issue(&plan)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let message = if consent.is_some() {
            "the consent token is not valid for this download; ask the user again"
        } else {
            "this firmware is downloaded; ask the user before fetching it"
        };
        return Err(RpcError::new(code::CONSENT_REQUIRED, message)
            .with_data(json!({ "token": token, "plan": plan })));
    }
    let result_plan = plan.clone();
    let job = ctx.subscribe(move |sub| async move {
        let _one_at_a_time = state.build_lock.lock().await;
        let dirs = esp32bench_build::Dirs::new(&state.builds_root, &plan);
        let r = esp32bench_build::fetch_image(&plan, &source, &dirs, |stream, line| {
            sub.send("build.log", json!({ "stream": stream, "line": line }));
        })
        .await;
        if r.is_ok() {
            esp32bench_build::write_stamp(&dirs.out, &plan);
        }
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
        let mut first = esp32bench_build::diagnostics::Finder::new(Some(&plan.path));
        let r = idf::run(&plan, &dirs, |stream, line| {
            first.line(&line);
            let mut params = json!({ "stream": stream, "line": line });
            if let Some(level) = esp32bench_build::diagnostics::line_level(&line) {
                params["level"] = json!(level);
                if let Some(at) = first.location(&line) {
                    params["location"] = json!(at);
                }
            }
            sub.send("build.log", params);
        })
        .await;
        if r.is_ok() {
            esp32bench_build::write_stamp(&dirs.out, &plan);
        }
        let event = finished_event(r, first.first());
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
        "sources": esp32bench_build::idf::sources(&project.path),
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
        let watch = esp32bench_toolchain::DownloadWatch::start(&env, id);
        let install = esp32bench_toolchain::install(&env, id, |line| {
            sub.send("toolchain.progress", json!({ "line": line }));
        });
        tokio::pin!(install);
        // How far the download is, each second, while it lasts.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut last = None;
        let r = loop {
            tokio::select! {
                r = &mut install => break r,
                _ = tick.tick(), if watch.is_some() => {
                    if let Some(w) = &watch {
                        let bytes = w.bytes();
                        if last != Some(bytes) {
                            last = Some(bytes);
                            sub.send("toolchain.download", json!({ "bytes": bytes, "total": w.total }));
                        }
                    }
                }
            }
        };
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
            let locals = gdb.locals().await?;
            Ok(json!({
                "frames": frames.as_array().into_iter().flatten().map(stop_frame).collect::<Vec<_>>(),
                "locals": locals,
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

/// The simulated time a pin's rate (sim.pins' rate) is measured over.
const RATE_WINDOW_NS: u64 = 2_000_000_000;

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
    let mut explain = esp32bench_library::explain::Explainer::new(&manifest.explain);
    // What the project should print (the manifest's expect), checked as the output
    // comes (sim.expect). Not while debugging: the run pauses at the user's will.
    let expect = (!p.debug)
        .then(|| manifest.project(&p.project))
        .flatten()
        .and_then(|x| x.expect.as_ref())
        .and_then(esp32bench_library::expect::Expect::from_value);
    let mut checker = expect
        .as_ref()
        .map(esp32bench_library::expect::Checker::new)
        .filter(|c| !c.is_empty());
    // A board prints it all within `seconds`; the simulator gets twice that (four
    // times with chip-like timing, which runs slower) and five seconds to boot.
    let expect_wait = expect.as_ref().map(|e| {
        std::time::Duration::from_secs(u64::from(e.seconds) * if p.icount { 4 } else { 2 } + 5)
    });
    let expect_event = move |progress: &esp32bench_library::expect::Progress, timed_out: bool| {
        let e = expect.as_ref();
        json!({
            "matched": progress.matched,
            "missing": progress.missing,
            "needs": e.map(|e| e.needs.clone()),
            "seconds": e.map(|e| e.seconds),
            "timedOut": timed_out,
        })
    };
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
            let mut rates: HashMap<u8, esp32bench_sim::trace::Rate> = HashMap::new();
            let mut pin_state: std::collections::BTreeMap<u8, (Option<bool>, Option<bool>)> =
                std::collections::BTreeMap::new();
            let mut pin_changed: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
            // Edges since the last sim.trace event, and how many did not fit in it.
            let mut trace_batch: Vec<Value> = Vec::new();
            let mut trace_skipped: u64 = 0;
            let mut pin_flush = tokio::time::Instant::now();
            // Lines of numbers, for the app's plotter.
            let mut plot = esp32bench_serial::plot::Lines::new();
            let mut style = esp32bench_serial::style::Styler::new();
            // The expectation, said once at the start; its deadline, until it is met
            // or has passed.
            let mut expect_deadline = None;
            if let (Some(c), Some(wait)) = (&checker, expect_wait) {
                sub.send("sim.expect", expect_event(&c.progress(), false));
                expect_deadline = Some(tokio::time::Instant::now() + wait);
            }
            loop {
                let wait = display.as_ref().map(esp32bench_sim::display::Display::interval);
                tokio::select! {
                    () = async {
                        match expect_deadline {
                            Some(d) => tokio::time::sleep_until(d).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        expect_deadline = None;
                        if let Some(c) = &checker {
                            sub.send("sim.expect", expect_event(&c.progress(), true));
                        }
                    }
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
                                // The host sends it twice a second.
                                PinEvent::Clock { ns } => {
                                    sub.send("sim.clock", json!({ "ns": ns }));
                                    // How fast each pin toggles, over the last two
                                    // simulated seconds: sent when it changes, null
                                    // once a pin stops.
                                    let changed: Vec<Value> = {
                                        let t = trace.lock().unwrap_or_else(|p| p.into_inner());
                                        pin_state
                                            .keys()
                                            .filter_map(|&g| {
                                                let now = t.rate(g, ns, RATE_WINDOW_NS);
                                                let before = rates.get(&g).copied();
                                                let same = match (now, before) {
                                                    (Some(a), Some(b)) => !a.differs(&b),
                                                    (None, None) => true,
                                                    _ => false,
                                                };
                                                if same {
                                                    return None;
                                                }
                                                match now {
                                                    Some(r) => rates.insert(g, r),
                                                    None => rates.remove(&g),
                                                };
                                                let (level, output) = pin_state[&g];
                                                let mut o = json!({ "gpio": g, "rate": now });
                                                if let Some(l) = level {
                                                    o["level"] = json!(l);
                                                }
                                                if let Some(out) = output {
                                                    o["output"] = json!(out);
                                                }
                                                Some(o)
                                            })
                                            .collect()
                                    };
                                    if !changed.is_empty() {
                                        sub.send("sim.pins", json!({ "pins": changed }));
                                    }
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
                            let mut params = json!({
                                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                                "text": String::from_utf8_lossy(&bytes),
                            });
                            styled(&mut params, style.feed(&bytes));
                            let rows = plot.feed(&bytes);
                            if !rows.is_empty() {
                                params["plot"] = json!(rows);
                            }
                            let found = explain.feed(&bytes);
                            if !found.is_empty() {
                                params["explain"] = json!(found);
                            }
                            let sent = sub.send("sim.output", params);
                            if !sent {
                                break;
                            }
                            if let Some(progress) = checker.as_mut().and_then(|c| c.feed(&bytes)) {
                                if progress.complete() {
                                    expect_deadline = None;
                                }
                                if expect_deadline.is_some() || progress.complete() {
                                    sub.send("sim.expect", expect_event(&progress, false));
                                }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsoleSaveParams {
    path: PathBuf,
    text: String,
}

/// The most a saved console may hold: the UI keeps 10 000 lines, far below this.
const CONSOLE_SAVE_MAX: usize = 64 * 1024 * 1024;

/// Write a console's text (the serial monitor's or the simulator's, as the app shows
/// it) to `path`, which the user chose. The UI does no file access of its own.
async fn console_save(params: Value) -> Result<Value, RpcError> {
    let p: ConsoleSaveParams = parse_params(params)?;
    if !p.path.is_absolute() {
        return Err(RpcError::invalid_params("path must be absolute"));
    }
    if p.text.len() > CONSOLE_SAVE_MAX {
        return Err(RpcError::invalid_params("the text is too long to save"));
    }
    tokio::fs::write(&p.path, p.text.as_bytes())
        .await
        .map_err(|e| RpcError::new(code::FAILED, format!("writing {}: {e}", p.path.display())))?;
    Ok(json!({ "path": p.path, "bytes": p.text.len() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlotSaveParams {
    path: PathBuf,
    rows: Vec<Vec<esp32bench_serial::plot::Value>>,
}

/// Most plot rows plot.save takes.
const PLOT_SAVE_MAX: usize = 100_000;

/// Write plot rows (as serial.data and sim.output carry them) to `path` as CSV.
async fn plot_save(params: Value) -> Result<Value, RpcError> {
    let p: PlotSaveParams = parse_params(params)?;
    if !p.path.is_absolute() {
        return Err(RpcError::invalid_params("path must be absolute"));
    }
    if p.rows.len() > PLOT_SAVE_MAX {
        return Err(RpcError::invalid_params("too many rows to save"));
    }
    let text = esp32bench_serial::plot::csv(&p.rows);
    tokio::fs::write(&p.path, text.as_bytes())
        .await
        .map_err(|e| RpcError::new(code::FAILED, format!("writing {}: {e}", p.path.display())))?;
    Ok(json!({ "path": p.path, "rows": p.rows.len() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameSaveParams {
    path: PathBuf,
    width: u32,
    height: u32,
    data: String,
}

/// Write a picture (as sim.frame carries one) to `path` as PNG.
async fn frame_save(params: Value) -> Result<Value, RpcError> {
    use base64::Engine;
    let p: FrameSaveParams = parse_params(params)?;
    if !p.path.is_absolute() {
        return Err(RpcError::invalid_params("path must be absolute"));
    }
    if p.width > 4096 || p.height > 4096 {
        return Err(RpcError::invalid_params("the picture is too big"));
    }
    let rgb = base64::engine::general_purpose::STANDARD
        .decode(&p.data)
        .map_err(|e| RpcError::invalid_params(format!("data: {e}")))?;
    let png = esp32bench_sim::display::png(p.width, p.height, &rgb)
        .ok_or_else(|| RpcError::invalid_params("data is not width * height * 3 bytes"))?;
    tokio::fs::write(&p.path, &png)
        .await
        .map_err(|e| RpcError::new(code::FAILED, format!("writing {}: {e}", p.path.display())))?;
    Ok(json!({ "path": p.path, "bytes": png.len() }))
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

/// Parts for the simulator from a library project's wiring for a board
/// (`esp32bench_circuit::suggest`), and the parts it names that have no model. Nothing
/// is saved: the app offers them, and `circuit.set` places them. A project that is not
/// in the library (the user's own) gets none.
async fn circuit_suggest(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: CircuitRef = parse_params(params)?;
    check_ids(&p.project, &p.board)?;
    let manifest = state
        .library
        .load()
        .await
        .map_err(|e| RpcError::new(code::FAILED, e.to_string()))?;
    let project = manifest.as_ref().and_then(|m| m.project(&p.project));
    let build = project.and_then(|proj| proj.builds.iter().find(|b| b.board == p.board));
    let hints: Vec<&str> = project
        .map(|proj| proj.parts.iter().map(|x| x.name.as_str()).collect())
        .unwrap_or_default();
    let wiring: Vec<esp32bench_circuit::suggest::Wire> = build
        .map(|b| {
            b.wiring
                .iter()
                .map(|w| esp32bench_circuit::suggest::Wire {
                    from: &w.from,
                    to: &w.to,
                })
                .collect()
        })
        .unwrap_or_default();
    let s = esp32bench_circuit::suggest::from_wiring(&wiring, &hints);
    Ok(json!({ "circuit": s.circuit, "unmatched": s.unmatched }))
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
    #[serde(default)]
    reconnect: bool,
    /// A library project whose expected output to check this board's against.
    #[serde(default)]
    expect: Option<String>,
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
fn board_job(
    ctx: &Ctx,
    state: Arc<State>,
    opts: esp32bench_flash::Options,
    op: BoardOp,
    adapter: Option<esp32bench_device::Port>,
) -> u64 {
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
                    if let (Some(port), Some(chip)) = (&adapter, done["info"]["chip"].as_str()) {
                        // Best effort: not being able to remember it fails nothing.
                        let _ = state.known_boards.remember(port, chip);
                    }
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
    // What is found is remembered for this adapter, so its port names the chip next
    // time (device.list's identified).
    let adapter = list_ports(true)
        .await
        .ok()
        .and_then(|ports| ports.into_iter().find(|x| Path::new(&x.path) == p.port));
    let job = board_job(&ctx, state, opts, BoardOp::Inspect { scratch }, adapter);
    Ok(json!({ "job": job }))
}

async fn board_erase(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: EraseParams = parse_params(params)?;
    if !p.confirm {
        return Err(RpcError::invalid_params("erasing needs confirm: true"));
    }
    check_port(&state, &p.port).await?;
    let opts = board_options(&p.port, p.reset, p.stub);
    let job = board_job(&ctx, state, opts, BoardOp::Erase, None);
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
    let job = board_job(&ctx, state, opts, BoardOp::Flash { image, chip }, None);
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

/// The site's explanations of output lines (the manifest's explain), for a run.
async fn explainer(state: &State) -> esp32bench_library::explain::Explainer {
    let rules = match state.library.load().await {
        Ok(Some(m)) => m.explain,
        _ => Vec::new(),
    };
    esp32bench_library::explain::Explainer::new(&rules)
}

async fn serial_open(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialOpenParams = parse_params(params)?;
    check_port(&state, &p.path).await?;
    let mut explain = explainer(&state).await;
    let (port, mut rx) = {
        let path = p.path.clone();
        tokio::task::spawn_blocking(move || esp32bench_serial::Port::open(&path, p.baud))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .map_err(serial_error)?
    };
    // What the project flashed on this board should print (serial.expect), checked
    // as on the simulator but on the board's own time: its seconds and five more.
    let expect = match &p.expect {
        Some(id) => match state.library.load().await {
            Ok(Some(m)) => m
                .project(id)
                .and_then(|x| x.expect.as_ref())
                .and_then(esp32bench_library::expect::Expect::from_value),
            _ => None,
        },
        None => None,
    };
    let new_checker = {
        let expect = expect.clone();
        move || {
            expect
                .as_ref()
                .map(esp32bench_library::expect::Checker::new)
                .filter(|c| !c.is_empty())
        }
    };
    let expect_wait = expect
        .as_ref()
        .map(|e| std::time::Duration::from_secs(u64::from(e.seconds) + 5));
    let expect_event = move |progress: &esp32bench_library::expect::Progress, timed_out: bool| {
        let e = expect.as_ref();
        json!({
            "matched": progress.matched,
            "missing": progress.missing,
            "needs": e.map(|e| e.needs.clone()),
            "seconds": e.map(|e| e.seconds),
            "timedOut": timed_out,
        })
    };
    let port: SharedPort = Arc::new(std::sync::Mutex::new(port));
    let conn = ctx.connection_id();
    let (path, baud, reconnect) = (p.path.clone(), p.baud, p.reconnect);
    let id = ctx.subscribe(move |sub| {
        // Registered before the task can run, and removed by `entry` when the task
        // ends or is aborted, so the table never holds a closed subscription's port.
        let key = (conn, sub.id());
        state
            .serial_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, port.clone());
        let recording: SharedRecording = Arc::default();
        state
            .serial_records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, recording.clone());
        let entry = PortEntry {
            state: state.clone(),
            key,
        };
        async move {
            let _entry = entry;
            use base64::Engine;
            // Lines of numbers, for the app's plotter.
            let mut plot = esp32bench_serial::plot::Lines::new();
            let mut style = esp32bench_serial::style::Styler::new();
            let mut garble = esp32bench_serial::style::Garble::default();
            let mut checker = new_checker();
            let mut expect_deadline = None;
            if let (Some(c), Some(wait)) = (&checker, expect_wait) {
                sub.send("serial.expect", expect_event(&c.progress(), false));
                expect_deadline = Some(tokio::time::Instant::now() + wait);
            }
            loop {
                let event = tokio::select! {
                    e = rx.recv() => match e {
                        Some(e) => e,
                        None => break,
                    },
                    () = async {
                        match expect_deadline {
                            Some(d) => tokio::time::sleep_until(d).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        expect_deadline = None;
                        if let Some(c) = &checker {
                            sub.send("serial.expect", expect_event(&c.progress(), true));
                        }
                        continue;
                    }
                };
                match event {
                    esp32bench_serial::Event::Data(bytes) => {
                        // Recording: the bytes as they came. A failed write ends it.
                        let failed = {
                            let mut r = recording.lock().unwrap_or_else(|p| p.into_inner());
                            match r.as_mut().map(|rec| rec.write(&bytes)) {
                                Some(Err(e)) => {
                                    let rec = r.take();
                                    Some((e, rec.map_or(0, |rec| rec.bytes())))
                                }
                                _ => None,
                            }
                        };
                        if let Some((e, written)) = failed {
                            sub.send(
                                "serial.recordFailed",
                                json!({ "message": e.to_string(), "bytes": written }),
                            );
                        }
                        let mut params = json!({
                            "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                            "text": String::from_utf8_lossy(&bytes),
                        });
                        styled(&mut params, style.feed(&bytes));
                        let progress = checker.as_mut().and_then(|c| c.feed(&bytes));
                        // Unreadable output: most likely the wrong speed.
                        let garbled = garble.feed(&bytes);
                        let rows = plot.feed(&bytes);
                        if !rows.is_empty() {
                            params["plot"] = json!(rows);
                        }
                        let found = explain.feed(&bytes);
                        if !found.is_empty() {
                            params["explain"] = json!(found);
                        }
                        let sent = sub.send("serial.data", params);
                        if !sent {
                            break;
                        }
                        if garbled {
                            let mut hint = json!({ "baud": baud });
                            if baud != 115_200 {
                                hint["suggest"] = json!(115_200);
                            }
                            sub.send("serial.garbled", hint);
                        }
                        if let Some(progress) = progress {
                            if progress.complete() {
                                expect_deadline = None;
                            }
                            if expect_deadline.is_some() || progress.complete() {
                                sub.send("serial.expect", expect_event(&progress, false));
                            }
                        }
                    }
                    esp32bench_serial::Event::Closed(reason) => {
                        // A board that resets over its own USB, or is plugged back
                        // in, comes back at the same path: open it again.
                        if reconnect {
                            if !sub.send("serial.lost", json!({ "reason": reason })) {
                                break;
                            }
                            if let Some((again, again_rx)) = reopen_port(&state, &path, baud).await
                            {
                                *port.lock().unwrap_or_else(|p| p.into_inner()) = again;
                                rx = again_rx;
                                plot = esp32bench_serial::plot::Lines::new();
                                style = esp32bench_serial::style::Styler::new();
                                garble.reset();
                                if !sub.send("serial.reconnected", json!({})) {
                                    break;
                                }
                                // It came back from a reset: it starts again.
                                checker = new_checker();
                                if let (Some(c), Some(wait)) = (&checker, expect_wait) {
                                    sub.send("serial.expect", expect_event(&c.progress(), false));
                                    expect_deadline = Some(tokio::time::Instant::now() + wait);
                                }
                                continue;
                            }
                        }
                        sub.send("serial.closed", json!({ "reason": reason }));
                        break;
                    }
                }
            }
        }
    });
    Ok(json!({ "subscription": id }))
}

/// Add a chunk's text without colour codes (`shown`) and its finished lines' levels
/// and tags (`lines`) to serial.data's or sim.output's params.
fn styled(params: &mut Value, s: esp32bench_serial::style::Styled) {
    params["shown"] = json!(s.text);
    if !s.lines.is_empty() {
        params["lines"] = json!(s.lines);
    }
}

/// Open `path` again once it is back, trying every quarter second for
/// `state.serial_reconnect_for`; None if it does not come back in time.
async fn reopen_port(
    state: &State,
    path: &Path,
    baud: u32,
) -> Option<(
    esp32bench_serial::Port,
    tokio::sync::mpsc::UnboundedReceiver<esp32bench_serial::Event>,
)> {
    let until = tokio::time::Instant::now() + state.serial_reconnect_for;
    while tokio::time::Instant::now() < until {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if check_port(state, path).await.is_err() {
            continue;
        }
        let p = path.to_owned();
        if let Ok(Ok(opened)) =
            tokio::task::spawn_blocking(move || esp32bench_serial::Port::open(&p, baud)).await
        {
            return Some(opened);
        }
    }
    None
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialRecordParams {
    subscription: u64,
    #[serde(default)]
    path: Option<PathBuf>,
}

/// serial.record: with a path, append everything the board sends from now on to that
/// file (the user chose it); without, stop. Returns the file and the bytes written.
async fn serial_record(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: SerialRecordParams = parse_params(params)?;
    let slot = state
        .serial_records
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(ctx.connection_id(), p.subscription))
        .cloned()
        .ok_or_else(|| {
            RpcError::new(
                code::NOT_FOUND,
                format!("no open serial port {} on this connection", p.subscription),
            )
        })?;
    let started = match &p.path {
        Some(path) => {
            let path = path.clone();
            let r = tokio::task::spawn_blocking(move || {
                esp32bench_serial::record::Recording::start(&path)
            })
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?;
            Some(r.map_err(|e| match e.kind() {
                std::io::ErrorKind::InvalidInput => RpcError::invalid_params(e.to_string()),
                _ => RpcError::new(code::FAILED, e.to_string()),
            })?)
        }
        None => None,
    };
    let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
    let previous = std::mem::replace(&mut *slot, started);
    let (path, bytes) = match (slot.as_ref(), previous.as_ref()) {
        (Some(now), _) => (Some(now.path().to_owned()), 0),
        (None, Some(was)) => (Some(was.path().to_owned()), was.bytes()),
        (None, None) => (None, 0),
    };
    Ok(json!({ "recording": slot.is_some(), "path": path, "bytes": bytes }))
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

/// The ports and the advice on them (`esp32bench_device::advice`).
async fn list_with_advice(
    state: &State,
    all: bool,
) -> Result<(Vec<esp32bench_device::Port>, Vec<esp32bench_device::Advice>), RpcError> {
    let mut ports = list_ports(all).await?;
    state.known_boards.apply(&mut ports);
    let for_advice = ports.clone();
    let advice = tokio::task::spawn_blocking(move || esp32bench_device::advice(&for_advice))
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    Ok((ports, advice))
}

async fn device_list(params: Value, state: Arc<State>) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    let (ports, advice) = list_with_advice(&state, p.all).await?;
    Ok(json!({ "ports": ports, "advice": advice }))
}

async fn device_watch(params: Value, ctx: Ctx, state: Arc<State>) -> Result<Value, RpcError> {
    let p: DeviceListParams = parse_params(params)?;
    // Start monitoring before listing, so a change in between is not missed.
    let mut monitor = esp32bench_device::Monitor::new()
        .map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))?;
    let (ports, advice) = list_with_advice(&state, p.all).await?;
    let mut last = (ports.clone(), advice.clone());
    let id = ctx.subscribe(move |sub| async move {
        // Events go out only after the monitor's settle delay (300 ms), so in
        // practice the client has the subscription id before the first one arrives.
        while monitor.changed().await.is_ok() {
            let Ok(now) = list_with_advice(&state, p.all).await else {
                continue;
            };
            if now == last {
                continue;
            }
            if !sub.send("device.changed", json!({ "ports": now.0, "advice": now.1 })) {
                break;
            }
            last = now;
        }
    });
    Ok(json!({ "subscription": id, "ports": ports, "advice": advice }))
}

/// Holds the daemon's lock file; the lock is released when this is dropped.
#[cfg(unix)]
pub struct InstanceLock {
    _file: std::fs::File,
    socket: PathBuf,
}

#[cfg(unix)]
impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Take the single-instance lock next to `socket`, then bind the socket (mode 0600).
/// Fails if another daemon holds the lock. A stale socket left by a crashed daemon is
/// replaced, which is safe because the lock proves nobody is serving it.
#[cfg(unix)]
pub fn bind(socket: &Path) -> io::Result<(Listener, InstanceLock)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = socket.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "socket path has no directory")
    })?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    let lock_path = dir.join("core.lock");
    let file = std::fs::File::options()
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
    let listener = Listener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok((
        listener,
        InstanceLock {
            _file: file,
            socket: socket.to_owned(),
        },
    ))
}

/// On Windows the pipe itself is the lock: creating its first instance fails while
/// another daemon has it (esp32bench_rpc::transport).
#[cfg(windows)]
pub struct InstanceLock;

#[cfg(windows)]
pub fn bind(socket: &Path) -> io::Result<(Listener, InstanceLock)> {
    let listener = Listener::bind(socket).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "another esp32benchd is running ({} is taken)",
                    socket.display()
                ),
            )
        } else {
            e
        }
    })?;
    Ok((listener, InstanceLock))
}
