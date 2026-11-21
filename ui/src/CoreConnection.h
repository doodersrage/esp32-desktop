// The UI's link to the core daemon: connects, checks the protocol with core.hello,
// reconnects when the core goes away, and can start esp32benchd. Widgets use the
// typed request methods here and listen for the result signals; they never build
// JSON-RPC requests themselves.

#pragma once

#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QList>
#include <QObject>
#include <QString>

#include <functional>

class QTimer;
class RpcClient;
struct RpcReply;

class CoreConnection : public QObject {
    Q_OBJECT

public:
    enum class State {
        Disconnected,  // No core reachable; retrying in the background.
        Connecting,    // Socket open, waiting for core.hello.
        Connected,     // Handshake done; requests can be sent.
        Incompatible,  // The core speaks a protocol this UI does not understand.
    };
    Q_ENUM(State)

    // Protocol major version this UI understands (schema/rpc/index.json).
    // The protocol this program speaks (schema/rpc/index.json). A core with the same
    // major and this minor or newer will do; an older one lacks methods we call.
    static constexpr int ProtocolMajor = 0;
    static constexpr int ProtocolMinor = 26;

    explicit CoreConnection(QObject *parent = nullptr);

    // Socket path: $ESP32BENCH_SOCKET, else $XDG_RUNTIME_DIR/esp32bench/core.sock.
    static QString defaultSocketPath();

    void setSocketPath(const QString &path);
    // Start the core once if the first connection attempt finds none (the app does
    // this; tests leave it off). Later disconnects are not restarted automatically.
    void setAutoStart(bool on) { m_autoStart = on; }
    QString socketPath() const { return m_socketPath; }

    void start();          // Begin connecting, and keep retrying while disconnected.
    State state() const { return m_state; }
    QString coreVersion() const { return m_coreVersion; }
    QString lastError() const { return m_lastError; }

    // Launch esp32benchd detached, so it outlives the UI. Returns false if no daemon
    // binary was found ($ESP32BENCH_DAEMON, next to this program, or on PATH).
    bool startCore();
    // Process id of the core this object last started, or 0.
    qint64 startedCorePid() const { return m_startedCorePid; }

    // device.list: one answer through devicesListed.
    void listDevices(bool includeNonUsb = false);
    // device.watch: the current list now, and again through devicesListed every time
    // a port appears or disappears, until the connection drops.
    void watchDevices();

    // library.list: the cached project library through libraryListed.
    void listLibrary();
    // library.refresh: fetch the library; libraryRefreshed reports the outcome, and a
    // successful refresh is followed by libraryListed.
    void refreshLibrary();
    // library.star, then the library is listed again.
    void starProject(const QString &id, bool starred);
    // library.get: one project with its boards, through projectLoaded.
    void getProject(const QString &id);
    // What an esp32bench:// link opens (link.open): answers with linkOpened.
    void openLink(const QString &url);

    // Local ESP-IDF projects (local.*). openLocal answers with localOpened, listLocal
    // with localListed, getLocal with localLoaded; removeLocal lists them again.
    void openLocal(const QString &path);
    void listLocal();
    void getLocal(const QString &id);
    void removeLocal(const QString &id);

    // build.start. Without a consent token the core refuses, and buildConsentRequired
    // carries the plan to show the user; call again with the token if they agree.
    void startBuild(const QString &project, const QString &board, const QString &consent = {});
    // job.cancel for the running build.
    void cancelBuild();
    bool isBuilding() const { return m_buildJob != 0; }

    // serial.open: one port at a time. serialOpened, then serialData until
    // serialClosed (unplugged, closed, or the connection dropped).
    void openSerial(const QString &path, int baud);
    void closeSerial();
    void writeSerial(const QByteArray &bytes);
    void resetSerial();
    bool isSerialOpen() const { return m_serialSub > 0; }

    // toolchain.status through toolchainStatus; toolchain.install as a job reporting
    // through toolchainProgress and toolchainInstallFinished.
    void requestToolchainStatus();
    void installComponent(const QString &component);
    bool isInstalling() const { return m_installJob != 0; }

    // board.inspect / board.flash / board.erase: one board job at a time, reporting
    // through boardProgress and boardFinished. board.flash asks for consent first
    // (boardConsentRequired); call flashBuild again with the token if the user agrees.
    void inspectBoard(const QString &port);
    void flashBuild(const QString &port, const QString &project, const QString &board,
                    const QString &consent = {});
    void eraseBoard(const QString &port);
    bool isBoardBusy() const { return m_boardJob != 0; }

    // sim.start: run the last build of a project for a board in QEMU. simStarted, then
    // simOutput until simStopped. One simulator at a time.
    // With debug, QEMU starts paused with GDB attached: debugStarted follows simStarted,
    // then the debug* calls below work until the simulator stops.
    void startSim(const QString &project, const QString &board, bool debug = false);
    // Whether simulations started from now on count time in instructions (sim.start's
    // icount): chip-like timing, independent of this computer's load, but slower.
    void setSimIcount(bool on) { m_simIcount = on; }
    bool simIcount() const { return m_simIcount; }
    void stopSim();
    void writeSim(const QByteArray &bytes);
    void resetSim();
    bool isSimRunning() const { return m_simSub > 0; }
    bool isDebugging() const { return m_simSub > 0 && m_debugging; }
    // The running simulation's bridged GPIOs (sim.start's `pins`); empty when it has
    // no pin access. setSimPin drives one of them.
    QList<int> simPins() const { return m_simPins; }
    void setSimPin(int gpio, bool level);
    // sim.traceSave: write the signal trace of the running simulation, or of the last
    // one if none runs, to `path` as VCD. traceSaved or requestFailed follows.
    void saveTrace(const QString &path);
    // Whether there is a simulation whose trace saveTrace can write: running or the
    // last one, with pin access.
    bool canSaveTrace() const { return m_traceSub > 0; }
    // Whether the running simulation's virtual parts work (sim.start's partsActive).
    bool simPartsActive() const { return m_simSub > 0 && m_simPartsActive; }
    // The running simulation's simulated Wi-Fi network (sim.start's network: ssid,
    // webUrl); empty when it has none.
    QJsonObject simNetwork() const { return m_simSub > 0 ? m_simNetwork : QJsonObject(); }
    // sim.control: work a part's control (a button's press, a switch's toggle, a
    // slider's number).
    void controlPart(const QString &id, const QString &control, const QJsonValue &value);
    // circuit.models: the kinds of part, through modelsLoaded.
    void loadModels();

    // circuit.get / circuit.set: the virtual parts on a project's board. Both answer
    // with circuitLoaded (set with what was saved).
    void loadCircuit(const QString &project, const QString &board);
    void saveCircuit(const QString &project, const QString &board, const QJsonObject &circuit);
    // circuit.suggest: the parts a library project's wiring names, with circuitSuggested.
    void suggestCircuit(const QString &project, const QString &board);
    // console.save: the core writes `text` to `path` (absolute); consoleSaved after.
    void saveConsole(const QString &path, const QString &text);
    // plot.save: the core writes plot rows to `path` as CSV; plotSaved after.
    void savePlot(const QString &path, const QJsonArray &rows);
    // library.export: the core saves the sketch for `board` under `dir`; sketchExported after.
    void exportSketch(const QString &project, const QString &board, const QString &dir);
    // build.status: whether the project is built for the board; buildStatusReady after.
    void requestBuildStatus(const QString &project, const QString &board);

    // debug.*: on the simulator started with debug. Results come as signals.
    void debugBreak(const QString &location);
    void debugDelete(int breakpoint);
    void debugControl(const QString &action);
    void debugInspect();

    // Error code the core uses when consent is missing (docs/rpc.md).
    static constexpr int ConsentRequired = -32002;

    RpcClient *client() const { return m_client; }

signals:
    // setAutoStart: the core was not running and could not be started.
    void autoStartFailed(const QString &message);
    void stateChanged(CoreConnection::State state);
    void devicesListed(const QJsonArray &ports);
    // device.list's advice on missing or unusable boards; sent just before devicesListed.
    void deviceAdvice(const QJsonArray &advice);
    // The library.list result: available, source, version, generated, fetchedAt, projects.
    void libraryListed(const QJsonObject &library);
    void libraryRefreshed(bool ok, const QString &error);
    // The library.get result: {project, boards}.
    void projectLoaded(const QJsonObject &result);
    // A folder opened as a local project (local.open's project).
    void localOpened(const QJsonObject &project);
    // local.list: the projects, and the ESP-IDF install (empty when there is none).
    void localListed(const QJsonArray &projects, const QJsonObject &idf);
    // local.get's result.
    void localLoaded(const QJsonObject &result);
    // A link named this project, and this board if the project has a build for it.
    void linkOpened(const QString &project, const QString &board);
    // Builds.
    void buildConsentRequired(const QString &project, const QString &board, const QString &token,
                              const QJsonObject &plan);
    void buildStarted(const QJsonObject &plan);
    void buildLog(const QString &stream, const QString &line);
    // `image` is the merged flash image when ok.
    void buildFinished(bool ok, const QString &error, const QString &image);
    // Serial monitor.
    void serialOpened(const QString &path, int baud);
    void serialData(const QByteArray &bytes);
    void serialClosed(const QString &reason);
    // Toolchain.
    void toolchainStatus(const QJsonArray &components);
    void toolchainProgress(const QString &line);
    void toolchainInstallFinished(bool ok, const QString &error);
    // Boards. `operation` is "inspect", "flash" or "erase".
    void boardConsentRequired(const QString &port, const QString &project, const QString &board,
                              const QString &token, const QJsonObject &plan);
    void boardStarted(const QString &operation, const QString &port);
    void boardProgress(const QString &stage, qint64 written, qint64 total);
    // `result` has `info`, `partitions` and `written` where the job produced them.
    void boardFinished(const QString &operation, bool ok, const QString &error, const QJsonObject &result);
    // Simulator.
    void simStarted(const QString &project, const QString &board, const QString &chip);
    void simOutput(const QByteArray &bytes);
    // sim.wifiJoined: the core gave a library sketch the simulated network's settings.
    void simWifiJoined(const QString &ssid);
    // Rows of numbers from the simulator's or the serial port's output (the events'
    // plot field): arrays of {name, value}.
    void plotRows(const QJsonArray &rows);
    // What lines of output mean (the events' explain field): `source` is build, serial
    // or sim; each finding has key, text, level (problem or info) and maybe url.
    void outputExplained(const QString &source, const QJsonArray &findings);
    // sim.expect: how much of what the project should print has appeared.
    void simExpect(const QStringList &matched, const QStringList &missing, const QString &needs, int seconds,
                   bool timedOut);
    // A new picture on the simulator's virtual RGB panel (sim.frame).
    void simFrame(const QImage &image);
    // sim.pins: pins that changed, each {gpio, level?, output?}.
    void simPinsChanged(const QJsonArray &pins);
    // sim.trace: every pin change since the last, in order, each {t (ns), gpio, level,
    // input?}; `skipped` more did not fit in the event.
    void simTrace(const QJsonArray &edges, qint64 skipped);
    void traceSaved(const QString &path, qint64 edges, qint64 dropped);
    // sim.parts: parts whose state changed, each {id, on}.
    void simPartsChanged(const QJsonArray &parts);
    // sim.partPicture: a display part's picture, lit pixels white on black.
    void partPicture(const QString &id, const QImage &image);
    void circuitLoaded(const QString &project, const QString &board, const QJsonObject &circuit);
    void consoleSaved(const QString &path);
    void plotSaved(const QString &path, qint64 rows);
    void sketchExported(const QString &path, const QStringList &libraries);
    void buildStatusReady(const QString &project, const QString &board, const QJsonObject &status);
    // A failed build's first compiler error (build.finished's firstError); sent just
    // before buildFinished.
    void buildFirstError(const QJsonObject &error);
    // A port opened with reconnect went away; the core waits for it to come back.
    void serialLost(const QString &reason);
    void serialReconnected();
    // Parts to place (a circuit) and the parts named that have no model.
    void circuitSuggested(const QString &project, const QString &board, const QJsonObject &circuit,
                          const QStringList &unmatched);
    void modelsLoaded(const QJsonArray &models);
    void simStopped(const QString &reason);

    // Debugger.
    void debugStarted();
    void debugStopped(const QString &reason, const QJsonObject &frame, int breakpoint);
    void debugRunning();
    void debugExited();
    void breakpointSet(const QJsonObject &breakpoint);
    void breakpointDeleted(int number);
    void debugInspected(const QJsonArray &frames, const QJsonArray &registers);
    // A request failed. `method` is the RPC method name.
    void requestFailed(const QString &method, const QString &message);

private:
    void setState(State s);
    void tryConnect();
    void onSocketConnected();
    void onNotification(const QString &method, const QJsonObject &params);
    void finishLibraryRefresh(const QJsonObject &params);
    void handleBuildEvent(const QString &method, const QJsonObject &params);
    void handleSerialEvent(const QString &method, const QJsonObject &params);
    void handleInstallEvent(const QString &method, const QJsonObject &params);
    void startBoardJob(const QString &operation, const QString &method, const QJsonObject &params,
                       std::function<bool(const RpcReply &)> onRefused = {});
    void handleBoardEvent(const QString &method, const QJsonObject &params);
    void handleSimEvent(const QString &method, const QJsonObject &params);

    RpcClient *m_client;
    QTimer *m_retry;
    QString m_socketPath;
    QString m_coreVersion;
    QString m_lastError;
    State m_state = State::Disconnected;
    qint64 m_startedCorePid = 0;
    qint64 m_devicesSubscription = 0; // 0 when not watching
    qint64 m_libraryJob = 0;          // running library.refresh, -1 while starting, or 0
    // library.refreshed events that arrived before library.refresh's reply told us
    // our job id (docs/rpc.md allows that order).
    QList<QJsonObject> m_earlyLibraryEvents;
    qint64 m_buildJob = 0; // running build.start job, -1 while starting, or 0
    QList<QPair<QString, QJsonObject>> m_earlyBuildEvents;
    qint64 m_serialSub = 0; // open serial.open subscription, -1 while opening, or 0
    QList<QPair<QString, QJsonObject>> m_earlySerialEvents;
    qint64 m_installJob = 0; // running toolchain.install job, -1 while starting, or 0
    QList<QPair<QString, QJsonObject>> m_earlyInstallEvents;
    qint64 m_boardJob = 0; // running board job, -1 while starting, or 0
    QString m_boardOperation;
    QList<QPair<QString, QJsonObject>> m_earlyBoardEvents;
    qint64 m_simSub = 0; // running sim.start subscription, -1 while starting, or 0
    bool m_debugging = false;
    QList<int> m_simPins;
    // The simulation whose trace saveTrace writes: the running one, else the last
    // one with pin access; 0 for none.
    qint64 m_traceSub = 0;
    bool m_simPartsActive = false;
    QJsonObject m_simNetwork;
    bool m_simIcount = false;
    bool m_autoStart = false;
    bool m_autoStartTried = false;
    QList<QPair<QString, QJsonObject>> m_earlySimEvents;
};
