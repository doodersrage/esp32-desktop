#include "MainWindow.h"

#include "BoardInfoDialog.h"
#include "BuildPanel.h"
#include "ConsoleView.h"
#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "LibraryPanel.h"
#include "MessageBar.h"
#include "ProjectView.h"
#include "SerialMonitor.h"
#include "SetupDialog.h"
#include "SimulatorPanel.h"
#include "DebugPanel.h"
#include "BacktraceDialog.h"
#include "DiskSpaceDialog.h"
#include "CodeView.h"
#include "DisplayPanel.h"
#include "PinsPanel.h"
#include "PlotPanel.h"
#include "ProblemsPanel.h"
#include "Preferences.h"
#include "TracePanel.h"
#include "CircuitPanel.h"
#include "CommandPalette.h"

#include <algorithm>

#include <QAction>
#include <QApplication>
#include <QComboBox>
#include <QFileInfo>
#include <QDesktopServices>
#include <QStandardPaths>
#include <QDir>
#include <QDragEnterEvent>
#include <QDropEvent>
#include <QLineEdit>
#include <QMimeData>
#include <QSysInfo>
#include <QFileDialog>
#include <QToolTip>
#include <QUrl>
#include <QUrlQuery>
#include <QCloseEvent>
#include <QDockWidget>
#include <QIcon>
#include <QJsonArray>
#include <QLocale>
#include <QLabel>
#include <QMenu>
#include <QMenuBar>
#include <QToolButton>
#include <QMessageBox>
#include <memory>
#include <QProgressBar>
#include <QPushButton>
#include <QSettings>
#include <QShortcut>
#include <QSplitter>
#include <QStatusBar>
#include <QVBoxLayout>
#include <QToolBar>

using namespace Qt::StringLiterals;

namespace {
// Saved layouts from before this version are not restored: the default changed (the
// toolbar, and fewer panels at first).
constexpr int LayoutVersion = 1;
} // namespace

MainWindow::MainWindow(CoreConnection *core, QWidget *parent)
    : QMainWindow(parent)
    , m_core(core)
    , m_devices(nullptr)
    , m_devicesDock(new QDockWidget(tr("Devices"), this))
    , m_connection(new QLabel(this))
    , m_startCore(new QPushButton(tr("Start core"), this))
{
    setWindowTitle(tr("ESP32 Bench"));

    m_split = new QSplitter(Qt::Horizontal, this);
    m_split->setObjectName(u"librarySplit"_s);
    m_library = new LibraryPanel(core, m_split);
    m_project = new ProjectView(core, m_split);
    m_split->addWidget(m_library);
    m_split->addWidget(m_project);
    m_split->setStretchFactor(0, 1);
    m_split->setStretchFactor(1, 1);
    connect(m_library, &LibraryPanel::projectSelected, m_project, &ProjectView::showProject);
    // Open in Arduino IDE: saved once in the sketchbook (later changes kept), then
    // opened with whatever opens .ino files here.
    connect(m_project, &ProjectView::openInIdeRequested, this, [this](const QString &project, const QString &board) {
        m_openSketch = true;
        m_core->exportSketch(project, board, sketchbookDir(), true);
    });
    connect(core, &CoreConnection::sketchExported, this, [this](const QString &path, const QStringList &libraries, bool) {
        const QString folder = QFileInfo(path).absolutePath();
        if (std::exchange(m_openSketch, false)) {
            const QString libs = libraries.isEmpty()
                ? QString()
                : u" "_s + tr("Install these libraries in it too: %1.").arg(libraries.join(u", "_s));
            if (QDesktopServices::openUrl(QUrl::fromLocalFile(path)))
                m_messages->showNotice(tr("Opened %1.").arg(path) + libs, QUrl::fromLocalFile(folder).toString(),
                                       tr("Open the folder"), {});
            else
                m_messages->showError(tr("Nothing on this computer opens .ino files; install the Arduino IDE. "
                                         "The sketch is in %1.")
                                          .arg(folder));
            return;
        }
        const QString text = libraries.isEmpty()
            ? tr("Saved the sketch to %1.").arg(path)
            : tr("Saved the sketch to %1. In the Arduino IDE, install these libraries too: %2.")
                  .arg(path, libraries.join(u", "_s));
        m_messages->showNotice(text, QUrl::fromLocalFile(folder).toString(), tr("Open the folder"), {});
    });
    // Try Blink: select it, then Run in simulator, which builds it first (asking).
    connect(core, &CoreConnection::libraryListed, this, [this](const QJsonObject &library) {
        bool has = false;
        for (const QJsonValue &v : library.value(u"projects"_s).toArray())
            has |= v.toObject().value(u"id"_s).toString() == u"blink" && v.toObject().value(u"simulator"_s).toBool();
        m_project->setTryAvailable(has);
    });
    connect(m_project, &ProjectView::tryRequested, this, [this] {
        m_tryPending = true;
        m_library->selectProject(u"blink"_s);
    });
    // Errors stay above the library and project until closed or a new attempt starts.
    auto *centre = new QWidget(this);
    m_messages = new MessageBar(centre);
    auto *centreLayout = new QVBoxLayout(centre);
    centreLayout->setContentsMargins(0, 0, 0, 0);
    centreLayout->addWidget(m_messages);
    centreLayout->addWidget(m_split, 1);
    setCentralWidget(centre);
    // What the board or a tool printed means: problems in the bar, notes in the console.
    connect(core, &CoreConnection::outputExplained, this, [this](const QString &source, const QJsonArray &findings) {
        for (const QJsonValue &v : findings) {
            const QJsonObject f = v.toObject();
            const QString text = f.value(u"text"_s).toString();
            if (f.value(u"level"_s).toString() == u"problem")
                m_messages->showExplanation(text, f.value(u"url"_s).toString());
            else if (source == u"sim")
                m_sim->console()->appendNotice(tr("Note: %1").arg(text));
            else if (source == u"serial")
                m_serial->console()->appendNotice(tr("Note: %1").arg(text));
        }
    });
    // A newer release on the site (library.list's update): said once per version, until
    // closed.
    connect(core, &CoreConnection::libraryListed, this, [this](const QJsonObject &library) {
        const QJsonObject update = library.value(u"update"_s).toObject();
        const QString version = update.value(u"version"_s).toString();
        if (version.isEmpty() || version == m_noticeShown || !Preferences::releaseNotices()
            || QSettings().value(u"update/dismissed"_s).toString() == version || !m_messages->isHidden())
            return;
        m_noticeShown = version;
        m_messages->showNotice(tr("ESP32 Bench %1 is out; you have %2.").arg(version, QApplication::applicationVersion()),
                               update.value(u"url"_s).toString(), tr("Get it from esp32bench.dev"), version);
    });
    connect(m_messages, &MessageBar::dismissed, this,
            [](const QString &version) { QSettings().setValue(u"update/dismissed"_s, version); });
    connect(core, &CoreConnection::toolchainStatus, this,
            [this](const QJsonArray &components) { m_toolchain = components; });
    setAcceptDrops(true);
    // An error's Copy details: the message, the library project and board shown (they
    // are the site's, not the user's), and the versions.
    m_messages->setDetails([this] {
        QStringList lines;
        const QString id = m_project->currentProjectId();
        if (!id.isEmpty() && !id.startsWith(u"local-"_s))
            lines << u"- Project: %1 (%2)"_s.arg(id, m_project->currentBuildBoard());
        return (lines + versionLines()).join(u'\n');
    });
    // Esc closes the message above the library (dialogs keep their own Esc).
    auto *escape = new QShortcut(QKeySequence(Qt::Key_Escape), this);
    connect(escape, &QShortcut::activated, this, [this] {
        // In the Build dock, Esc stops a build that is running.
        if (m_buildPanel->isAncestorOf(QApplication::focusWidget()) && m_buildPanel->cancelRunning())
            return;
        if (!m_messages->isHidden())
            m_messages->dismiss();
    });
    connect(core, &CoreConnection::buildStarted, m_messages, &QWidget::hide);
    connect(core, &CoreConnection::boardStarted, m_messages, &QWidget::hide);
    // A build or flash can take minutes: when it ends with the window behind others, ask
    // for attention (the taskbar entry is highlighted; on macOS the Dock icon bounces).
    connect(core, &CoreConnection::buildFinished, this, [this] {
        if (!isActiveWindow())
            QApplication::alert(this);
    });
    connect(core, &CoreConnection::boardFinished, this, [this](const QString &op) {
        if (op == u"flash" && !isActiveWindow())
            QApplication::alert(this);
    });
    // The first compiler error, when the core found one, says more than the exit status.
    connect(core, &CoreConnection::buildFirstError, this, [this](const QJsonObject &e) { m_firstError = e; });
    connect(core, &CoreConnection::buildStarted, this, [this] { m_firstError = {}; });
    connect(core, &CoreConnection::buildFinished, this, [this](bool ok, const QString &error) {
        if (ok)
            return;
        const QJsonObject e = std::exchange(m_firstError, {});
        if (e.isEmpty()) {
            m_messages->showError(tr("The build failed: %1\nThe Build panel below has the whole log.").arg(error));
            return;
        }
        const QString where = tr("%1, line %2").arg(e.value(u"file"_s).toString()).arg(e.value(u"line"_s).toInt());
        const QString text = tr("The build failed at %1: %2").arg(where, e.value(u"message"_s).toString());
        const QString path = e.value(u"path"_s).toString();
        if (path.isEmpty())
            m_messages->showError(tr("%1\nThe Build panel below has the whole log.").arg(text));
        else
            m_messages->showError(text, QUrl::fromLocalFile(path).toString(), tr("Open the file"));
    });

    m_devicesDock->setObjectName(u"devicesDock"_s); // needed for saveState()
    m_devices = new DevicesPanel(core, m_devicesDock);
    m_devicesDock->setWidget(m_devices);
    addDockWidget(Qt::LeftDockWidgetArea, m_devicesDock);

    m_buildDock = new QDockWidget(tr("Build"), this);
    m_buildDock->setObjectName(u"buildDock"_s);
    m_buildPanel = new BuildPanel(core, m_buildDock);
    m_buildDock->setWidget(m_buildPanel);
    addDockWidget(Qt::BottomDockWidgetArea, m_buildDock);
    connect(core, &CoreConnection::buildStarted, m_buildDock, &QDockWidget::show);
    connect(core, &CoreConnection::buildStarted, m_buildDock, &QDockWidget::raise);
    // An error or warning line that names a place: a library sketch's line in its Code
    // tab, a local project's file in the user's editor.
    connect(m_buildPanel, &BuildPanel::locationActivated, this, &MainWindow::showLocation);
    // A run's output check ending (it matched, or timed out) while the window is
    // behind others asks for attention, as builds and flashes do; once per run.
    connect(core, &CoreConnection::simStarted, this, [this] { m_expectAlerted = false; });
    connect(core, &CoreConnection::simExpect, this,
            [this](const QStringList &matched, const QStringList &missing, const QString &, int, bool timedOut) {
                const bool ended = timedOut || (missing.isEmpty() && !matched.isEmpty());
                if (ended && !std::exchange(m_expectAlerted, true) && !isActiveWindow())
                    QApplication::alert(this);
            });

    // Every explanation of the session, beside the build log; hidden until opened
    // from the View menu or the message bar.
    m_problemsDock = new QDockWidget(tr("Problems"), this);
    m_problemsDock->setObjectName(u"problemsDock"_s);
    m_problems = new ProblemsPanel(core, m_problemsDock);
    m_problemsDock->setWidget(m_problems);
    connect(m_problems, &ProblemsPanel::locationActivated, this, &MainWindow::showLocation);
    addDockWidget(Qt::BottomDockWidgetArea, m_problemsDock);
    tabifyDockWidget(m_buildDock, m_problemsDock);
    m_problemsDock->hide();
    // Its tab says how many there are, so new ones show while it is behind another.
    connect(m_problems, &ProblemsPanel::countChanged, this, [this](int n) {
        m_problemsDock->setWindowTitle(n > 0 ? tr("Problems (%1)").arg(n) : tr("Problems"));
    });

    m_serialDock = new QDockWidget(tr("Serial monitor"), this);
    m_serialDock->setObjectName(u"serialDock"_s);
    m_serial = new SerialMonitor(core, m_serialDock);
    m_serialDock->setWidget(m_serial);
    addDockWidget(Qt::BottomDockWidgetArea, m_serialDock);
    tabifyDockWidget(m_buildDock, m_serialDock);

    m_simDock = new QDockWidget(tr("Simulator"), this);
    m_simDock->setObjectName(u"simDock"_s);
    m_sim = new SimulatorPanel(core, m_simDock);
    // A decoded crash backtrace's line, clicked in a console.
    connect(m_sim->console(), &ConsoleView::frameActivated, this, &MainWindow::showLocation);
    connect(m_serial->console(), &ConsoleView::frameActivated, this, &MainWindow::showLocation);
    m_simDock->setWidget(m_sim);
    addDockWidget(Qt::BottomDockWidgetArea, m_simDock);
    tabifyDockWidget(m_serialDock, m_simDock);
    connect(core, &CoreConnection::simStarted, this, [this] {
        m_simDock->show();
        m_simDock->raise();
    });

    // The pins' signal trace, beside the simulator's console; there (not raised) when
    // a simulation has pin access.
    m_traceDock = new QDockWidget(tr("Trace"), this);
    m_traceDock->setObjectName(u"traceDock"_s);
    m_trace = new TracePanel(core, m_traceDock);
    m_traceDock->setWidget(m_trace);
    addDockWidget(Qt::BottomDockWidgetArea, m_traceDock);
    tabifyDockWidget(m_simDock, m_traceDock);
    m_simDock->raise();
    m_traceDock->hide();
    connect(core, &CoreConnection::simStarted, this, [this] {
        if (!m_core->simPins().isEmpty())
            m_traceDock->show();
        m_simDock->raise();
    });

    // Numbers the firmware prints, as lines; beside the console, shown when they come.
    m_plotDock = new QDockWidget(tr("Plot"), this);
    m_plotDock->setObjectName(u"plotDock"_s);
    m_plot = new PlotPanel(core, m_plotDock);
    m_plotDock->setWidget(m_plot);
    addDockWidget(Qt::BottomDockWidgetArea, m_plotDock);
    tabifyDockWidget(m_traceDock, m_plotDock);
    m_plotDock->hide();
    connect(m_plot, &PlotPanel::firstRows, this, [this] { m_plotDock->show(); });

    // The virtual RGB panel, on the right where the board would sit; it appears when
    // firmware first draws on it.
    m_displayDock = new QDockWidget(tr("Display"), this);
    m_displayDock->setObjectName(u"displayDock"_s);
    m_display = new DisplayPanel(core, m_displayDock);
    m_displayDock->setWidget(m_display);
    addDockWidget(Qt::RightDockWidgetArea, m_displayDock);
    m_displayDock->hide();
    connect(m_display, &DisplayPanel::firstFrame, this, [this] {
        m_displayDock->show();
        m_displayDock->raise();
        // Wide enough for the picture at its own size, if the window allows.
        const int want = std::min(m_display->sizeHint().width(), width() / 2);
        if (m_displayDock->width() < want)
            resizeDocks({m_displayDock}, {want}, Qt::Horizontal);
    });

    // The project's pins, on the right with the display; shown when a simulation has
    // pin access.
    m_pinsDock = new QDockWidget(tr("Pins"), this);
    m_pinsDock->setObjectName(u"pinsDock"_s);
    m_pins = new PinsPanel(core, m_pinsDock);
    m_pinsDock->setWidget(m_pins);
    // A pin double-clicked there: in the trace, selected, its latest change in view.
    connect(m_pins, &PinsPanel::pinActivated, this, [this](int gpio) {
        m_traceDock->show();
        m_traceDock->raise();
        m_trace->view()->showPin(gpio);
    });
    addDockWidget(Qt::RightDockWidgetArea, m_pinsDock);
    m_pinsDock->hide();
    connect(core, &CoreConnection::simStarted, this, [this] {
        if (!m_core->simPins().isEmpty())
            m_pinsDock->show();
        if (m_core->simPartsActive()) {
            m_circuitDock->show();
            m_circuitDock->raise();
        }
    });

    // Virtual parts on the selected project's board, on the right above the pins.
    m_circuitDock = new QDockWidget(tr("Circuit"), this);
    m_circuitDock->setObjectName(u"circuitDock"_s);
    m_circuit = new CircuitPanel(core, m_circuitDock);
    // An example for a part: shown as a link would show it.
    connect(m_circuit, &CircuitPanel::exampleRequested, this, [this](const QString &project, const QString &board) {
        m_project->preferBoard(board);
        m_project->showProject(project);
        m_library->selectProject(project);
    });
    m_circuitDock->setWidget(m_circuit);
    addDockWidget(Qt::RightDockWidgetArea, m_circuitDock);
    // One tab group on the right, so the canvas gets the full height: Circuit, Pins
    // and Display. Each is raised when it has something to show.
    tabifyDockWidget(m_circuitDock, m_pinsDock);
    tabifyDockWidget(m_circuitDock, m_displayDock);
    m_circuitDock->raise();
    connect(m_project, &ProjectView::boardSelected, m_circuit, &CircuitPanel::showBoard);
    connect(m_project, &ProjectView::buildCurrent, m_buildPanel, &BuildPanel::setBuildCurrent);
    // A simulation's pins named by the project's wiring ("GPIO 2 · LED anode").
    connect(core, &CoreConnection::simStarted, this, [this](const QString &project, const QString &board) {
        const QHash<int, QString> labels = m_project->pinLabels(project, board);
        m_pins->setLabels(labels);
        m_trace->view()->setLabels(labels);
    });
    connect(m_project, &ProjectView::buildCurrent, this, &MainWindow::updateToolBar);
    connect(m_library, &LibraryPanel::projectForgotten, this, [this](const QString &name, const QString &path) {
        m_messages->showUndo(tr("Forgot %1. Its folder is untouched.").arg(name), tr("Undo"),
                             [this, path] { m_core->openLocal(path); });
    });
    // A port opened by itself (reopened at start, or after a flash) shows its output.
    connect(core, &CoreConnection::serialOpened, m_serialDock, &QDockWidget::show);

    m_debugDock = new QDockWidget(tr("Debugger"), this);
    m_debugDock->setObjectName(u"debugDock"_s);
    m_debugger = new DebugPanel(core, m_debugDock);
    m_debugDock->setWidget(m_debugger);
    // Beside the simulator's console, not tabbed over it: both are needed at once.
    // (splitDockWidget would only add a tab, since the console is in a tab group.)
    addDockWidget(Qt::BottomDockWidgetArea, m_debugDock, Qt::Horizontal);
    m_debugDock->hide(); // until the first debug session
    connect(core, &CoreConnection::debugStarted, this, [this] {
        // A layout saved while the debugger was tabbed over the console: move it out.
        if (tabifiedDockWidgets(m_debugDock).contains(m_simDock)) {
            removeDockWidget(m_debugDock);
            addDockWidget(Qt::BottomDockWidgetArea, m_debugDock, Qt::Horizontal);
        }
        m_simDock->show();
        m_simDock->raise();
        m_debugDock->show();
        m_debugDock->raise();
    });
    connect(m_devices, &DevicesPanel::portActivated, this, &MainWindow::openSerialPort);
    connect(core, &CoreConnection::buildConsentRequired, this,
            [this](const QString &project, const QString &board, const QString &token, const QJsonObject &plan) {
                if (confirmBuild(project, board, plan))
                    m_core->startBuild(project, board, token);
                else
                    m_buildPanel->cancelPending(); // nor what was to follow it
            });

    m_boardInfo = new BoardInfoDialog(this);
    // Flashing needs the port to itself; close the monitor and reopen it after.
    connect(m_buildPanel, &BuildPanel::flashRequested, this,
            [this](const QString &port, const QString &project, const QString &board) {
                freePort(port);
                m_flashedProject = project;
                m_flashedBoard = board;
                m_core->flashBuild(port, project, board);
            });
    connect(core, &CoreConnection::boardConsentRequired, this,
            [this](const QString &port, const QString &project, const QString &board, const QString &token,
                   const QJsonObject &plan) {
                if (confirmFlash(port, plan))
                    m_core->flashBuild(port, project, board, token);
                else
                    m_reopenSerial.clear();
            });
    connect(m_devices, &DevicesPanel::inspectRequested, this, &MainWindow::inspectPort);
    connect(m_devices, &DevicesPanel::eraseRequested, this, &MainWindow::erasePort);
    connect(core, &CoreConnection::boardStarted, this, [this](const QString &, const QString &port) {
        m_lastBoardPort = port;
    });
    connect(core, &CoreConnection::boardFinished, this,
            [this](const QString &op, bool ok, const QString &error, const QJsonObject &result) {
                if (op == u"inspect" && ok) {
                    statusBar()->clearMessage();
                    // The core remembered the chip for this adapter: list the ports again.
                    m_core->listDevices();
                    m_boardInfo->showResult(m_lastBoardPort, result);
                    m_boardInfo->show();
                    m_boardInfo->raise();
                } else if (!ok) {
                    statusBar()->clearMessage();
                    m_messages->showError(op == u"flash" ? tr("Flashing failed: %1").arg(error)
                                                         : tr("Board operation failed: %1").arg(error));
                } else if (op == u"erase") {
                    statusBar()->showMessage(tr("Flash erased."), 10000);
                }
                // After flashing, show what the new firmware prints.
                if (op == u"flash" && ok) {
                    m_serialDock->show();
                    m_serialDock->raise();
                    // At the speed the new firmware talks, or its output is garbled.
                    m_serial->suggestBaud(m_serial->projectBaud(m_flashedProject));
                    // And whether it prints what the project expects.
                    // And decode its crashes from the build flashed.
                    m_serial->openPort(m_lastBoardPort, m_flashedProject, m_flashedBoard);
                    m_reopenSerial.clear();
                } else if (!m_reopenSerial.isEmpty()) {
                    m_serial->openPort(std::exchange(m_reopenSerial, {}));
                }
            });

    m_setup = new SetupDialog(core, this);
    // First run: if something builds need is missing, show the setup once.
    connect(core, &CoreConnection::linkOpened, this, [this](const QString &project, const QString &board) {
        m_project->preferBoard(board);
        m_project->showProject(project);
        m_library->selectProject(project);
        raise();
        activateWindow();
        statusBar()->showMessage(
            tr("Opened %1 from a link. Building or flashing it will ask first.").arg(project), 15000);
    });
    connect(core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method == u"library.export") {
            m_openSketch = false;
            m_messages->showError(tr("Could not save the sketch: %1").arg(message));
        }
        if (method == u"plot.save")
            m_messages->showError(tr("Could not save the plot: %1").arg(message));
        if (method == u"link.open")
            m_messages->showError(tr("Could not open the link: %1").arg(message));
    });
    connect(core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        if (s == CoreConnection::State::Connected) {
            // A link names what to show; otherwise reopen what was shown last time.
            if (m_pendingLinks.isEmpty() && !m_reopened && Preferences::reopenProject()) {
                QSettings settings;
                const QString last = settings.value(u"project/last"_s).toString();
                if (!last.isEmpty() && m_project->currentProjectId().isEmpty()) {
                    m_project->preferBoard(settings.value(u"project/lastBoard"_s).toString());
                    m_library->selectProject(last);
                }
            }
            m_reopened = true;
            for (const QString &url : std::exchange(m_pendingLinks, {}))
                m_core->openLink(url);
        }
        if (s != CoreConnection::State::Connected)
            return;
        QSettings settings;
        if (settings.value(u"setup/offered"_s).toBool())
            return;
        auto conn = std::make_shared<QMetaObject::Connection>();
        *conn = connect(m_core, &CoreConnection::toolchainStatus, this, [this, conn](const QJsonArray &components) {
            disconnect(*conn);
            for (const QJsonValue &v : components) {
                const QJsonObject c = v.toObject();
                if (c.value(u"neededFor"_s).toString() == u"build" && c.value(u"state"_s).toString() != u"installed") {
                    QSettings().setValue(u"setup/offered"_s, true);
                    showSetup();
                    return;
                }
            }
        });
        m_core->requestToolchainStatus();
    });

    m_startCore->setIcon(QIcon::fromTheme(u"media-playback-start"_s));
    m_startCore->setToolTip(tr("Start the esp32bench core in the background"));
    connect(m_startCore, &QPushButton::clicked, this, &MainWindow::startCore);
    connect(m_core, &CoreConnection::autoStartFailed, this, [this](const QString &message) {
        m_autoStartError = message;
        showConnectionState();
    });
    statusBar()->addWidget(m_connection, 1);
    // The simulation and the serial port, seen while their docks are closed.
    m_simState = new QLabel(statusBar());
    m_simState->setContentsMargins(6, 0, 6, 0);
    m_simState->hide();
    m_portState = new QLabel(statusBar());
    m_portState->setContentsMargins(6, 0, 6, 0);
    m_portState->hide();
    statusBar()->addPermanentWidget(m_simState);
    statusBar()->addPermanentWidget(m_portState);
    statusBar()->addPermanentWidget(m_startCore);
    const auto simState = [this] {
        if (!m_core->isSimRunning()) {
            m_simState->hide();
            return;
        }
        const QString what = m_core->isDebugging() ? tr("Debugging") : m_core->isSimPaused() ? tr("Paused") : tr("Simulating");
        m_simState->setText(m_simTime.isEmpty() ? what : tr("%1 · %2").arg(what, m_simTime));
        m_simState->show();
    };
    connect(m_core, &CoreConnection::simStarted, this, [this, simState] {
        m_simTime.clear();
        m_lastClockNs = -1;
        m_simSpeed.clear();
        simState();
    });
    connect(m_core, &CoreConnection::simStopped, this, simState);
    for (auto signal : {&CoreConnection::debugStarted, &CoreConnection::debugExited})
        connect(m_core, signal, this, simState);
    connect(m_core, &CoreConnection::simPaused, this, simState);
    connect(m_core, &CoreConnection::simClock, this, [this, simState](qint64 ns) {
        m_simTime = tr("%1 s simulated").arg(QLocale().toString(double(ns) / 1e9, 'f', 1));
        // How fast it runs: simulated time over this computer's, since the last tick.
        if (m_lastClockNs >= 0 && m_clockWall.isValid() && !m_core->isSimPaused()) {
            const qint64 wall = m_clockWall.nsecsElapsed();
            if (wall > 0)
                m_simSpeed = speedText(double(ns - m_lastClockNs) / double(wall));
        }
        m_lastClockNs = ns;
        m_clockWall.start();
        if (!m_simSpeed.isEmpty())
            m_simTime = tr("%1 · %2").arg(m_simTime, m_simSpeed);
        simState();
    });
    connect(m_core, &CoreConnection::serialOpened, this, [this](const QString &path, int baud) {
        m_portState->setText(tr("Serial: %1 at %2 baud").arg(path).arg(baud));
        m_portState->show();
    });
    connect(m_core, &CoreConnection::serialClosed, m_portState, &QWidget::hide);

    // At first only the simulator's console is open below; the build log and the serial
    // monitor open when a build starts or a port is opened.
    m_buildDock->hide();
    m_serialDock->hide();
    // The toolbar picks the port and its Board menu has what the Devices panel's
    // context menu has; the panel, with every port's details, is in the View menu.
    m_devicesDock->hide();

    buildToolBar();
    buildMenus();
    showShortcutsInToolTips();
    connect(m_core, &CoreConnection::stateChanged, this, &MainWindow::showConnectionState);
    showConnectionState();
    m_defaultState = saveState(LayoutVersion);
    restoreLayout();
}

void MainWindow::buildToolBar()
{
    m_toolBar = addToolBar(tr("Main toolbar"));
    m_toolBar->setObjectName(u"mainToolBar"_s);
    m_toolBar->setMovable(false);
    m_toolBar->setFloatable(false);
    m_toolBar->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_toolBar->toggleViewAction()->setEnabled(false); // always there

    m_current = new QLabel(m_toolBar);
    m_current->setContentsMargins(6, 0, 12, 0);
    QFont bold = m_current->font();
    bold.setBold(true);
    m_current->setFont(bold);
    m_toolBar->addWidget(m_current);
    m_toolBar->addAction(m_project->buildAction());
    m_toolBar->addSeparator();
    m_toolBar->addAction(m_buildPanel->simulateAction());
    m_toolBar->addAction(m_buildPanel->debugAction());
    m_stopSim = m_toolBar->addAction(QIcon::fromTheme(u"media-playback-stop"_s), tr("Stop"));
    m_stopSim->setToolTip(tr("Stop the simulation"));
    m_stopSim->setShortcut(QKeySequence(Qt::SHIFT | Qt::Key_F5));
    connect(m_stopSim, &QAction::triggered, m_core, &CoreConnection::stopSim);
    m_toolBar->insertAction(m_stopSim, m_sim->pauseAction());
    m_restartSim = new QAction(QIcon::fromTheme(u"view-refresh"_s), tr("Restart"), this);
    m_restartSim->setToolTip(tr("Stop the simulation and start the same build again, debugged if it was (%1)")
                                 .arg(QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_F5).toString(QKeySequence::NativeText)));
    m_restartSim->setShortcut(QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_F5));
    connect(m_restartSim, &QAction::triggered, m_core, &CoreConnection::restartSim);
    m_toolBar->addSeparator();
    auto *portLabel = new QLabel(tr("Port:"), m_toolBar);
    portLabel->setContentsMargins(6, 0, 4, 0);
    portLabel->setBuddy(m_buildPanel->portPicker());
    m_toolBar->addWidget(portLabel);
    m_toolBar->addWidget(m_buildPanel->portPicker());
    // What the Devices panel's context menu does, for the port chosen here.
    m_boardMenu = new QToolButton(m_toolBar);
    m_boardMenu->setIcon(QIcon::fromTheme(u"application-menu"_s, QIcon::fromTheme(u"preferences-system"_s)));
    m_boardMenu->setText(tr("Board"));
    m_boardMenu->setToolTip(tr("Serial monitor, inspect and erase for the board on the selected port"));
    m_boardMenu->setPopupMode(QToolButton::InstantPopup);
    auto *boardMenu = new QMenu(m_boardMenu);
    const auto port = [this] { return m_buildPanel->portPicker()->currentData().toString(); };
    boardMenu->addAction(QIcon::fromTheme(u"utilities-terminal"_s), tr("Open in serial monitor"), this,
                         [this, port] { openSerialPort(port()); });
    boardMenu->addAction(QIcon::fromTheme(u"document-properties"_s), tr("Inspect board…"), this,
                         [this, port] { inspectPort(port()); });
    boardMenu->addSeparator();
    boardMenu->addAction(QIcon::fromTheme(u"edit-delete"_s), tr("Erase flash…"), this,
                         [this, port] { erasePort(port()); });
    boardMenu->addSeparator();
    boardMenu->addAction(m_devicesDock->toggleViewAction());
    m_boardMenu->setMenu(boardMenu);
    m_toolBar->addWidget(m_boardMenu);
    m_toolBar->addAction(m_buildPanel->flashAction());
    // Busy while a build or a board operation runs; the Build panel has the details.
    m_busy = new QProgressBar(m_toolBar);
    m_busy->setRange(0, 0);
    m_busy->setMaximumWidth(120);
    m_busy->setTextVisible(false);
    m_busyAction = m_toolBar->addWidget(m_busy);
    auto *spacer = new QWidget(m_toolBar);
    spacer->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    m_toolBar->addWidget(spacer);
    m_nextStep = new QLabel(m_toolBar);
    m_nextStep->setContentsMargins(12, 0, 6, 0);
    m_nextStep->setTextFormat(Qt::RichText);
    connect(m_nextStep, &QLabel::linkActivated, this, &MainWindow::showSetup);
    m_toolBar->addWidget(m_nextStep);

    connect(m_project, &ProjectView::recentRequested, m_library, &LibraryPanel::selectProject);
    // Double-click or Enter in the library: run it, once it is shown (Run builds first,
    // asking; a project the simulator cannot run is built).
    connect(m_library, &LibraryPanel::projectActivated, this, [this](const QString &id) {
        m_activatePending = id;
        runActivated();
    });
    connect(m_project, &ProjectView::currentChanged, this, &MainWindow::runActivated);
    // The serial monitor's quick-send lines follow the project shown.
    connect(m_project, &ProjectView::currentChanged, this,
            [this] { m_serial->setProject(m_project->currentProjectId()); });
    // So do the kept breakpoints, shown beside the sketch's lines; a click beside a
    // line adds or removes one there (set at once while paused, else kept for the
    // next debug run).
    connect(m_project, &ProjectView::currentChanged, this,
            [this] { m_debugger->setProject(m_project->currentProjectId()); });
    connect(m_debugger, &DebugPanel::breakpointsChanged, this, &MainWindow::showBreakpointLines);
    connect(m_project, &ProjectView::currentChanged, this, &MainWindow::showBreakpointLines);
    // A build's errors and warnings in the sketch shown, marked on its lines.
    connect(m_core, &CoreConnection::buildStarted, this, [this] {
        m_codeProblems.clear();
        m_project->codeView()->setProblems({});
    });
    connect(m_core, &CoreConnection::buildLog, this,
            [this](const QString &, const QString &line, const QString &level, const QJsonObject &at) {
                const QString id = m_project->currentProjectId();
                if (level.isEmpty() || id.isEmpty() || at.value(u"file"_s).toString() != id + u".ino"_s)
                    return;
                const int n = at.value(u"line"_s).toInt();
                // An error over a warning on the same line.
                if (n <= 0 || (m_codeProblems.contains(n) && m_codeProblems.value(n).first == u"error"))
                    return;
                m_codeProblems.insert(n, {level, line.trimmed()});
                m_project->codeView()->setProblems(m_codeProblems);
            });
    connect(m_project, &ProjectView::currentChanged, this, [this] {
        // Another sketch: its lines are not those.
        if (m_project->currentProjectId() != m_problemsFor) {
            m_problemsFor = m_project->currentProjectId();
            m_codeProblems.clear();
            m_project->codeView()->setProblems({});
        }
    });
    connect(m_project->codeView(), &CodeView::functionBreakRequested, this, [this](const QString &function) {
        if (!m_debugger->toggleBreakpoint(function)) {
            m_debugDock->show();
            m_debugDock->raise();
        }
    });
    connect(m_project, &ProjectView::conditionRequested, this, [this](int line) {
        m_debugger->editCondition(u"%1.ino:%2"_s.arg(m_project->currentProjectId()).arg(line));
    });
    // While paused, the value of the name the mouse rests on, as a tooltip.
    connect(m_project->codeView(), &CodeView::wordHovered, this, [this](const QString &word, const QPoint &at) {
        m_hoverWord = word;
        m_hoverAt = at;
        m_core->debugEvaluate(word);
    });
    connect(m_core, &CoreConnection::debugEvaluated, this,
            [this](const QString &expression, const QString &value, const QString &error) {
                if (expression != m_hoverWord)
                    return;
                m_hoverWord.clear();
                if (error.isEmpty())
                    QToolTip::showText(m_hoverAt, tr("%1 = %2").arg(expression, value), m_project->codeView());
            });
    connect(m_project, &ProjectView::runToLineRequested, this, [this](int line) {
        m_debugger->runTo(u"%1.ino:%2"_s.arg(m_project->currentProjectId()).arg(line));
    });
    // Run to a line works while paused in a debug run.
    connect(m_core, &CoreConnection::debugStopped, this, [this] { m_project->codeView()->setRunToEnabled(true); });
    for (auto signal : {&CoreConnection::debugRunning, &CoreConnection::debugExited})
        connect(m_core, signal, this, [this] { m_project->codeView()->setRunToEnabled(false); });
    connect(m_core, &CoreConnection::simStopped, this, [this] { m_project->codeView()->setRunToEnabled(false); });
    connect(m_project, &ProjectView::codeLineClicked, this, [this](int line) {
        const QString id = m_project->currentProjectId();
        if (!m_debugger->toggleBreakpoint(u"%1.ino:%2"_s.arg(id).arg(line))) {
            m_debugDock->show();
            m_debugDock->raise();
        }
    });
    // Paused in the sketch shown: its line is marked in the Code tab; and the line of a
    // caller's frame when one is picked in the call stack.
    const auto markFrame = [this](const QJsonObject &frame) {
        const QString id = m_project->currentProjectId();
        const bool here = !id.isEmpty() && QFileInfo(frame.value(u"file"_s).toString()).fileName() == id + u".ino"_s;
        m_project->showPausedLine(here ? frame.value(u"line"_s).toInt() : 0);
    };
    connect(m_core, &CoreConnection::debugStopped, this,
            [markFrame](const QString &, const QJsonObject &frame) { markFrame(frame); });
    connect(m_debugger, &DebugPanel::frameSelected, this, markFrame);
    connect(m_debugger, &DebugPanel::sourceRequested, this, &MainWindow::showLocation);
    for (auto signal : {&CoreConnection::debugRunning, &CoreConnection::debugExited})
        connect(m_core, signal, this, [this] { m_project->showPausedLine(0); });
    connect(m_core, &CoreConnection::simStopped, this, [this] { m_project->showPausedLine(0); });
    // Another project picked meanwhile: that run is off.
    connect(m_library, &LibraryPanel::projectSelected, this, [this](const QString &id) {
        if (id != m_activatePending)
            m_activatePending.clear();
    });
    {
        QSettings settings;
        showRecentOnGuide(settings.value(u"project/recent"_s).toStringList(),
                          settings.value(u"project/recentTitles"_s).toStringList());
    }
    connect(m_project, &ProjectView::currentChanged, this, [this] {
        if (const QString id = m_project->currentProjectId(); !id.isEmpty()) {
            QSettings settings;
            settings.setValue(u"project/last"_s, id);
            settings.setValue(u"project/lastBoard"_s, m_project->currentBuildBoard());
            // Newest first, each once, at most eight.
            QStringList ids = settings.value(u"project/recent"_s).toStringList();
            QStringList titles = settings.value(u"project/recentTitles"_s).toStringList();
            while (titles.size() < ids.size())
                titles << ids.at(titles.size());
            if (const qsizetype at = ids.indexOf(id); at >= 0) {
                ids.removeAt(at);
                titles.removeAt(at);
            }
            ids.prepend(id);
            titles.prepend(m_project->currentProjectTitle());
            showRecentOnGuide(ids, titles);
            settings.setValue(u"project/recent"_s, ids.mid(0, 8));
            settings.setValue(u"project/recentTitles"_s, titles.mid(0, 8));
        }
        m_buildPanel->setTarget(m_project->currentProjectId(), m_project->currentBuildBoard(),
                                m_project->buildAction()->isEnabled());
        updateToolBar();
    });
    // Run, Debug or Flash on a project not built yet builds it first.
    connect(m_buildPanel, &BuildPanel::buildRequested, this, [this] {
        if (m_project->buildAction()->isEnabled())
            m_project->buildAction()->trigger();
        else
            m_buildPanel->cancelPending();
    });
    connect(m_buildPanel, &BuildPanel::actionsChanged, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::buildStarted, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::buildFinished, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::simStarted, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::simStopped, this, &MainWindow::updateToolBar);
    // Checked when either changes: the project view shows blink before the build panel
    // has it as its target.
    const auto tryNow = [this] {
        if (!m_tryPending || m_project->currentProjectId() != u"blink"
            || m_buildPanel->targetProject() != u"blink")
            return;
        if (m_buildPanel->simulateAction()->isEnabled()) {
            m_tryPending = false;
            m_buildPanel->simulateAction()->trigger();
        }
    };
    connect(m_project, &ProjectView::currentChanged, this, tryNow);
    connect(m_buildPanel, &BuildPanel::actionsChanged, this, tryNow);
    connect(m_core, &CoreConnection::stateChanged, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::boardStarted, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::boardFinished, this, &MainWindow::updateToolBar);
    // Whether builds can run: arduino-cli and the ESP32 core installed. Asked on every
    // connection and after an install.
    connect(m_core, &CoreConnection::toolchainStatus, this, [this](const QJsonArray &components) {
        m_buildToolsMissing = false;
        for (const QJsonValue &v : components) {
            const QJsonObject c = v.toObject();
            if (c.value(u"neededFor"_s).toString() == u"build" && c.value(u"state"_s).toString() != u"installed")
                m_buildToolsMissing = true;
        }
        updateToolBar();
    });
    connect(m_core, &CoreConnection::toolchainInstallFinished, m_core, &CoreConnection::requestToolchainStatus);
    connect(m_core, &CoreConnection::toolchainInstallFinished, this, &MainWindow::updateToolBar);
    connect(m_core, &CoreConnection::toolchainProgress, this, [this] {
        if (!m_busyAction->isVisible())
            updateToolBar();
    });
    connect(m_core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        if (s == CoreConnection::State::Connected)
            m_core->requestToolchainStatus();
    });
    updateToolBar();
}

void MainWindow::showShortcutsInToolTips()
{
    // A toolbar button's tooltip says its shortcut ("Build (Ctrl+B)"), and follows the
    // action's text (Build or Download).
    for (QAction *a : m_toolBar->actions()) {
        const auto update = [a] {
            if (a->shortcut().isEmpty())
                return;
            const QString tip = tr("%1 (%2)").arg(a->text().remove(u'&'), a->shortcut().toString(QKeySequence::NativeText));
            if (a->toolTip() != tip)
                a->setToolTip(tip);
        };
        update();
        connect(a, &QAction::changed, this, update);
    }
}

void MainWindow::updateToolBar()
{
    const QString title = m_project->currentProjectTitle();
    const QString board = m_project->currentBoardName();
    const QString current = title.isEmpty() ? tr("No project selected")
                            : board.isEmpty() ? title
                                              : tr("%1 · %2").arg(title, board);
    m_current->setText(m_current->fontMetrics().elidedText(current, Qt::ElideRight, 360));
    m_current->setToolTip(current);
    m_stopSim->setEnabled(m_core->isSimRunning());
    if (m_restartSim)
        m_restartSim->setEnabled(m_core->isSimRunning() && m_core->canRestartSim());
    if (m_rebuild)
        m_rebuild->setEnabled(m_project->buildAction()->isEnabled());
    if (m_decodeAction)
        m_decodeAction->setEnabled(!m_project->currentProjectId().isEmpty() && !m_project->currentBuildBoard().isEmpty());
    m_boardMenu->setEnabled(m_buildPanel->portPicker()->currentIndex() >= 0
                            && m_core->state() == CoreConnection::State::Connected && !m_core->isBoardBusy());
    m_busyAction->setVisible(m_core->isBuilding() || m_core->isBoardBusy() || m_core->isInstalling());

    setWindowTitle(title.isEmpty() ? tr("ESP32 Bench") : tr("%1 — ESP32 Bench").arg(title));
    const QString id = m_project->currentProjectId();
    const bool built = !id.isEmpty() && m_buildPanel->isBuilt(id, m_project->currentBuildBoard());
    QString next;
    if (m_core->state() != CoreConnection::State::Connected)
        next.clear();
    else if (m_core->isBuilding())
        next = tr("Building…");
    else if (m_core->isBoardBusy())
        next = tr("Working on the board…");
    else if (m_core->isInstalling())
        next = tr("Installing the build tools…");
    else if (m_core->isSimRunning())
        next = tr("Simulating. The console is below.");
    else if (title.isEmpty())
        next = tr("Next: pick a project.");
    else if (built)
        next = tr("Next: run it in the simulator, or flash it to a board.");
    // Your own ESP-IDF projects build with your ESP-IDF, not these tools.
    else if (m_buildToolsMissing && !id.startsWith(u"local-"))
        next = tr("First, <a href=\"setup\">install the build tools</a>.");
    else if (m_project->buildAction()->isEnabled())
        next = tr("Next: build it, or press Run in simulator, which builds it first.");
    m_nextStep->setText(next);
}

QString MainWindow::nextStepText() const
{
    return m_nextStep->text();
}

void MainWindow::applyDefaultSizes()
{
    resizeDocks({m_devicesDock, m_circuitDock}, {260, 380}, Qt::Horizontal);
    const int w = m_split->width() > 0 ? m_split->width() : 760;
    m_split->setSizes({w * 2 / 5, w * 3 / 5});
}

void MainWindow::resetLayout()
{
    restoreState(m_defaultState, LayoutVersion);
    applyDefaultSizes();
    m_library->resetColumns();
}

QString MainWindow::connectionText() const
{
    return m_connection->text();
}

void MainWindow::buildMenus()
{
    QMenu *file = menuBar()->addMenu(tr("&File"));
    QAction *openIdf = file->addAction(QIcon::fromTheme(u"document-open-folder"_s), tr("&Open ESP-IDF Project…"));
    openIdf->setShortcut(QKeySequence::Open);
    connect(openIdf, &QAction::triggered, m_library, &LibraryPanel::openFolder);
    // The projects shown lately, newest first (project/recent in the settings).
    QMenu *recent = file->addMenu(QIcon::fromTheme(u"document-open-recent"_s), tr("Recent projects"));
    connect(recent, &QMenu::aboutToShow, this, [this, recent] {
        recent->clear();
        QSettings settings;
        const QStringList ids = settings.value(u"project/recent"_s).toStringList();
        const QStringList titles = settings.value(u"project/recentTitles"_s).toStringList();
        for (int i = 0; i < ids.size(); ++i) {
            const QString id = ids.at(i);
            recent->addAction(i < titles.size() ? titles.at(i) : id, this, [this, id] {
                m_library->selectProject(id);
            });
        }
        if (ids.isEmpty())
            recent->addAction(tr("None yet"))->setEnabled(false);
    });
    file->addSeparator();
    // Your settings in one file, to move to another computer.
    file->addSeparator();
    file->addAction(tr("&Export settings…"), this, [this] {
        const QString path = QFileDialog::getSaveFileName(
            this, tr("Export your settings"),
            QDir(QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation)).filePath(u"esp32bench-settings.json"_s),
            tr("Settings files (*.json)"));
        if (!path.isEmpty())
            m_core->exportSettings(path);
    });
    file->addAction(tr("&Import settings…"), this, [this] {
        const QString path = QFileDialog::getOpenFileName(
            this, tr("Import settings"), QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation),
            tr("Settings files (*.json)"));
        if (!path.isEmpty())
            m_core->importSettings(path);
    });
    connect(m_core, &CoreConnection::settingsExported, this, [this](const QString &path, const QJsonObject &) {
        statusBar()->showMessage(tr("Your settings were saved to %1.").arg(path), 10000);
    });
    connect(m_core, &CoreConnection::settingsImported, this, [this](const QJsonObject &, int skipped) {
        statusBar()->showMessage(skipped > 0 ? tr("Settings imported; some entries did not fit this version and were left out.")
                                             : tr("Settings imported."),
                                 10000);
        // What they change, read again: stars, and the project shown.
        m_core->listLibrary();
        const QString id = m_project->currentProjectId();
        if (!id.isEmpty()) {
            m_debugger->setProject({});
            m_debugger->setProject(id);
            m_project->showProject(id);
        }
    });
    connect(m_core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method == u"data.export" || method == u"data.import")
            m_messages->showError(tr("Settings: %1").arg(message));
    });
    QAction *quit = file->addAction(QIcon::fromTheme(u"application-exit"_s), tr("&Quit"));
    quit->setShortcut(QKeySequence::Quit);
    connect(quit, &QAction::triggered, this, &QWidget::close);

    // The toolbar's steps, where their shortcuts show.
    QMenu *project = menuBar()->addMenu(tr("&Project"));
    project->addAction(m_project->buildAction());
    project->addAction(m_buildPanel->simulateAction());
    project->addAction(m_buildPanel->debugAction());
    project->addAction(m_sim->pauseAction());
    project->addAction(m_stopSim);
    project->addAction(m_restartSim);
    project->addSeparator();
    m_decodeAction = project->addAction(tr("&Decode a crash backtrace…"), this, &MainWindow::showBacktraceDialog);
    m_decodeAction->setToolTip(tr("Turn a backtrace from a saved log or a forum post into lines of the project shown"));
    // Rebuild from scratch: the project's build for this board removed, then built.
    m_rebuild = project->addAction(tr("Re&build from scratch"), this, [this] {
        const QString id = m_project->currentProjectId(), board = m_project->currentBuildBoard();
        if (id.isEmpty() || board.isEmpty())
            return;
        m_rebuildPending = id + u'/' + board;
        m_core->cleanCache(id, board);
    });
    m_rebuild->setToolTip(tr("Remove this project's build for the board shown, and build it again from nothing: "
                             "for a build that behaves oddly"));
    connect(m_core, &CoreConnection::cacheCleaned, this, [this](const QString &project, const QString &board) {
        if (project + u'/' + board != std::exchange(m_rebuildPending, {}))
            return;
        if (m_project->buildAction()->isEnabled())
            m_project->buildAction()->trigger();
    });
    QAction *nextBoard = project->addAction(tr("Next board"), this, [this] { m_project->stepBoard(1); });
    nextBoard->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_BracketRight));
    QAction *previousBoard = project->addAction(tr("Previous board"), this, [this] { m_project->stepBoard(-1); });
    previousBoard->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_BracketLeft));
    project->addAction(m_buildPanel->nextProblemAction());
    project->addAction(m_buildPanel->previousProblemAction());
    project->addSeparator();
    project->addAction(m_buildPanel->flashAction());

    QMenu *view = menuBar()->addMenu(tr("&View"));
    view->addAction(m_devicesDock->toggleViewAction());
    view->addAction(m_buildDock->toggleViewAction());
    view->addAction(m_problemsDock->toggleViewAction());
    view->addAction(m_serialDock->toggleViewAction());
    view->addAction(m_simDock->toggleViewAction());
    view->addAction(m_traceDock->toggleViewAction());
    view->addAction(m_plotDock->toggleViewAction());
    view->addAction(m_debugDock->toggleViewAction());
    view->addAction(m_displayDock->toggleViewAction());
    view->addAction(m_pinsDock->toggleViewAction());
    view->addAction(m_circuitDock->toggleViewAction());
    view->addSeparator();
    // Ctrl+K: any action or project by typing.
    QAction *palette = view->addAction(tr("Find an action or &project…"));
    palette->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_K));
    connect(palette, &QAction::triggered, this, &MainWindow::showCommandPalette);
    // Ctrl+F finds in the console being used, else in the library.
    QAction *find = view->addAction(tr("&Find…"));
    find->setShortcut(QKeySequence::Find);
    connect(find, &QAction::triggered, this, &MainWindow::find);
    view->addSeparator();
    // Alt+1 to Alt+9: straight to a panel, shown if it was closed.
    QMenu *goTo = view->addMenu(tr("&Go to"));
    const QList<std::pair<QString, QWidget *>> places = {
        {tr("Library"), nullptr},           {tr("Build"), m_buildDock},       {tr("Simulator"), m_simDock},
        {tr("Serial monitor"), m_serialDock}, {tr("Debugger"), m_debugDock},  {tr("Circuit"), m_circuitDock},
        {tr("Trace"), m_traceDock},         {tr("Pins"), m_pinsDock},         {tr("Problems"), m_problemsDock}};
    for (int i = 0; i < places.size(); ++i) {
        QWidget *place = places.at(i).second;
        QAction *a = goTo->addAction(places.at(i).first, this, [this, place] { goToPanel(place); });
        a->setShortcut(QKeySequence(Qt::ALT | Qt::Key(Qt::Key_1 + i)));
    }
    QMenu *layouts = view->addMenu(tr("&Layout"));
    layouts->addAction(tr("&Coding"), this, [this] { showLayout(Layout::Coding); });
    layouts->addAction(tr("&Debugging"), this, [this] { showLayout(Layout::Debugging); });
    layouts->addAction(tr("&Hardware"), this, [this] { showLayout(Layout::Hardware); });
    QAction *reset = view->addAction(QIcon::fromTheme(u"view-restore"_s), tr("&Reset layout"));
    reset->setToolTip(tr("Put the panels back where they were at first"));
    connect(reset, &QAction::triggered, this, &MainWindow::resetLayout);

    QMenu *tools = menuBar()->addMenu(tr("&Tools"));
    QAction *setup = tools->addAction(QIcon::fromTheme(u"configure"_s), tr("&Toolchain setup…"));
    connect(setup, &QAction::triggered, this, &MainWindow::showSetup);
    tools->addAction(tr("&Disk space…"), this, [this] {
                         if (!m_diskSpace)
                             m_diskSpace = new DiskSpaceDialog(m_core, this);
                         m_diskSpace->refresh();
                         m_diskSpace->show();
                         m_diskSpace->raise();
                     });
    QAction *prefs = tools->addAction(QIcon::fromTheme(u"preferences-system"_s), tr("&Preferences…"));
    prefs->setShortcut(QKeySequence::Preferences);
    prefs->setMenuRole(QAction::PreferencesRole); // in the app menu on macOS
    connect(prefs, &QAction::triggered, this, &MainWindow::showPreferences);

    QMenu *help = menuBar()->addMenu(tr("&Help"));
    help->addAction(QIcon::fromTheme(u"internet-web-browser"_s), tr("esp32bench.dev &guide"), this, [] {
        QDesktopServices::openUrl(QUrl(u"https://esp32bench.dev/desktop/"_s));
    });
    QAction *keys = help->addAction(tr("&Keyboard shortcuts"), this, &MainWindow::showShortcuts);
    keys->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_Slash));
    help->addAction(QIcon::fromTheme(u"tools-report-bug"_s), tr("&Report a problem…"), this, [this] {
        QDesktopServices::openUrl(problemReportUrl());
    });
    help->addSeparator();
    QAction *about = help->addAction(QIcon::fromTheme(u"help-about"_s), tr("&About ESP32 Bench"));
    connect(about, &QAction::triggered, this, [this] {
        QMessageBox::about(this, tr("About ESP32 Bench"),
                           tr("<p><b>ESP32 Bench</b> %1</p>"
                              "<p>A workbench and simulator for ESP32 boards, "
                              "companion to esp32bench.dev.</p>"
                              "<p>Core: %2</p>"
                              "<p>Licensed under the GNU GPL, version 2 or later.</p>")
                               .arg(QApplication::applicationVersion(),
                                    m_core->coreVersion().isEmpty() ? tr("not connected")
                                                                    : m_core->coreVersion()));
    });
}

void MainWindow::showConnectionState()
{
    using S = CoreConnection::State;
    const S s = m_core->state();
    switch (s) {
    case S::Connected:
        m_autoStartError.clear();
        m_connection->setText(tr("Connected to core %1").arg(m_core->coreVersion()));
        break;
    case S::Connecting:
        m_connection->setText(tr("Connecting to the core…"));
        break;
    case S::Incompatible:
        m_connection->setText(tr("Incompatible core: %1").arg(m_core->lastError()));
        break;
    case S::Disconnected:
        m_connection->setText(m_autoStartError.isEmpty()
                                  ? tr("The core is not running. Retrying…")
                                  : tr("The core is not running, and starting it failed: %1").arg(m_autoStartError));
        break;
    }
    m_startCore->setVisible(s == S::Disconnected);
}

void MainWindow::startCore()
{
    if (!m_core->startCore()) {
        QMessageBox::warning(this, tr("Could not start the core"),
                             tr("%1\n\nInstall esp32benchd, or set ESP32BENCH_DAEMON to its path.")
                                 .arg(m_core->lastError()));
    }
}

void MainWindow::restoreLayout()
{
    QSettings settings;
    const QByteArray geometry = settings.value(u"mainWindow/geometry"_s).toByteArray();
    if (geometry.isEmpty() || !restoreGeometry(geometry))
        resize(1400, 820);
    const QByteArray state = settings.value(u"mainWindow/state"_s).toByteArray();
    if (state.isEmpty() || !restoreState(state, LayoutVersion)) {
        applyDefaultSizes(); // first start, or a layout from an older version
        return;
    }
    m_split->restoreState(settings.value(u"mainWindow/librarySplit"_s).toByteArray());
    m_library->restoreColumns(settings.value(u"mainWindow/libraryColumns"_s).toByteArray());
}

void MainWindow::saveLayout()
{
    QSettings settings;
    settings.setValue(u"mainWindow/geometry"_s, saveGeometry());
    settings.setValue(u"mainWindow/state"_s, saveState(LayoutVersion));
    settings.setValue(u"mainWindow/librarySplit"_s, m_split->saveState());
    settings.setValue(u"mainWindow/libraryColumns"_s, m_library->saveColumns());
}

void MainWindow::closeEvent(QCloseEvent *event)
{
    saveLayout();
    QMainWindow::closeEvent(event);
}

bool MainWindow::confirmBuild(const QString &project, const QString &board, const QJsonObject &plan)
{
    // Prebuilt firmware: downloaded and checked, not built; nothing runs on this computer.
    if (const QJsonObject image = plan.value(u"image"_s).toObject(); !image.isEmpty()) {
        QMessageBox box(QMessageBox::Question, tr("Download prebuilt firmware?"),
                        tr("“%1” for %2 is ready-made firmware from esp32bench.dev. It is downloaded and "
                           "checked against the library's SHA-256, so the app can run it in the simulator "
                           "or flash it to a board, which asks again.")
                            .arg(project, board),
                        QMessageBox::Cancel, this);
        box.setInformativeText(tr("%1 will be downloaded.")
                                   .arg(QLocale().formattedDataSize(image.value(u"size"_s).toInteger())));
        box.setDetailedText(tr("From: %1\nSHA-256: %2")
                                .arg(image.value(u"url"_s).toString(), image.value(u"sha256"_s).toString()));
        QPushButton *download = box.addButton(tr("Download"), QMessageBox::AcceptRole);
        box.setDefaultButton(QMessageBox::Cancel);
        box.exec();
        return box.clickedButton() == download;
    }
    QStringList libs;
    for (const QJsonValue &v : plan.value(u"install"_s).toArray())
        libs << v.toString();
    QMessageBox box(QMessageBox::Warning, tr("Build a downloaded project?"),
                    tr("“%1” for %2 came from esp32bench.dev. Building it runs its code through "
                       "arduino-cli on this computer.")
                        .arg(project, board),
                    QMessageBox::Cancel, this);
    box.setInformativeText(libs.isEmpty()
                               ? tr("No libraries will be downloaded.")
                               : tr("These libraries will be downloaded and installed: %1.")
                                     .arg(libs.join(u", "_s)));
    box.setDetailedText(tr("Board: %1\nCode SHA-256: %2")
                            .arg(plan.value(u"fqbn"_s).toString(), plan.value(u"codeSha256"_s).toString()));
    QPushButton *build = box.addButton(tr("Build"), QMessageBox::AcceptRole);
    box.setDefaultButton(QMessageBox::Cancel);
    box.exec();
    return box.clickedButton() == build;
}

void MainWindow::openLink(const QString &url)
{
    if (url.isEmpty()) {
        raise();
        activateWindow();
        return;
    }
    if (m_core->state() == CoreConnection::State::Connected)
        m_core->openLink(url);
    else
        m_pendingLinks << url;
}

QString MainWindow::sketchbookDir()
{
    // Where the Arduino IDE keeps sketches unless told otherwise.
#ifdef Q_OS_LINUX
    return QDir::home().filePath(u"Arduino"_s);
#else
    return QDir(QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation)).filePath(u"Arduino"_s);
#endif
}

void MainWindow::runActivated()
{
    if (m_activatePending.isEmpty() || m_project->currentProjectId() != m_activatePending)
        return;
    QAction *run = m_buildPanel->simulateAction();
    QAction *build = m_project->buildAction();
    if (run->isEnabled()) {
        m_activatePending.clear();
        run->trigger();
    } else if (build->isEnabled()) {
        m_activatePending.clear();
        build->trigger();
    }
}

void MainWindow::showRecentOnGuide(const QStringList &ids, const QStringList &titles)
{
    QList<std::pair<QString, QString>> recent;
    for (qsizetype i = 0; i < ids.size(); ++i)
        recent.append({i < titles.size() ? titles.at(i) : ids.at(i), ids.at(i)});
    m_project->setRecent(recent);
}

QList<CommandPalette::Entry> MainWindow::paletteEntries() const
{
    QList<CommandPalette::Entry> out;
    // The menus' actions that can be used now.
    for (QAction *top : menuBar()->actions()) {
        QMenu *menu = top->menu();
        if (!menu)
            continue;
        const QString where = top->text().remove(u'&');
        for (QAction *a : menu->actions()) {
            if (a->isSeparator() || a->menu() || !a->isEnabled() || a->text().isEmpty())
                continue;
            const QString keys = a->shortcut().toString(QKeySequence::NativeText);
            out.append({a->text().remove(u'&'), keys.isEmpty() ? where : u"%1, %2"_s.arg(where, keys),
                        [a] { a->trigger(); }});
        }
    }
    // The projects, to open.
    for (const auto &[name, id] : m_library->projects()) {
        out.append({name, tr("Project"), [this, id] { m_library->selectProject(id); }});
    }
    return out;
}

void MainWindow::showCommandPalette()
{
    if (!m_palette)
        m_palette = new CommandPalette(this);
    m_palette->setEntries(paletteEntries());
    // Centred near the top of the window, as such palettes are.
    m_palette->move(mapToGlobal(QPoint((width() - m_palette->width()) / 2, 60)));
    m_palette->show();
    m_palette->field()->setFocus();
}

void MainWindow::sayWhatsNew()
{
    QSettings settings;
    const QString last = settings.value(u"app/lastVersion"_s).toString();
    const QString now = QApplication::applicationVersion();
    settings.setValue(u"app/lastVersion"_s, now);
    if (last.isEmpty() || last == now || !m_messages->isHidden())
        return;
    m_messages->showNotice(tr("ESP32 Bench is now %1 (it was %2).").arg(now, last),
                           u"https://github.com/doodersrage/esp32-desktop/releases/tag/v%1"_s.arg(now),
                           tr("See what's new"), {});
}

void MainWindow::showPreferences()
{
    if (!m_preferences) {
        m_preferences = new PreferencesDialog(this);
        // The consoles follow the text size as it changes, and go back on Cancel.
        const auto apply = [this](int steps) {
            m_serial->console()->setZoomSteps(steps);
            m_sim->console()->setZoomSteps(steps);
        };
        connect(m_preferences, &PreferencesDialog::textSizeChanged, this, apply);
        connect(m_preferences, &QDialog::rejected, this,
                [apply] { apply(QSettings().value(u"console/zoom"_s, 0).toInt()); });
    }
    m_preferences->load();
    m_preferences->show();
    m_preferences->raise();
}

void MainWindow::showLocation(const QJsonObject &at)
{
    const QString file = at.value(u"file"_s).toString();
    const int line = at.value(u"line"_s).toInt();
    const QString id = m_project->currentProjectId();
    if (!id.isEmpty() && file == id + u".ino"_s && m_project->showCodeLine(line))
        return;
    const QString path = at.value(u"path"_s).toString();
    if (!path.isEmpty()) {
        if (!QDesktopServices::openUrl(QUrl::fromLocalFile(path)))
            m_messages->showError(tr("Nothing on this computer opens %1. Its line %2 has the problem.")
                                      .arg(QDir::toNativeSeparators(path))
                                      .arg(line));
        return;
    }
    m_messages->showError(tr("Line %1 of %2 is not in this project's own code: it is in a library "
                             "or in the ESP32 core the build uses.")
                              .arg(line)
                              .arg(file));
}

QString MainWindow::speedText(double ratio)
{
    return tr("%1× real time").arg(QLocale().toString(std::clamp(ratio, 0.0, 99.0), 'f', 1));
}

QString MainWindow::simStateText() const
{
    return m_simState->isHidden() ? QString() : m_simState->text();
}

QString MainWindow::portStateText() const
{
    return m_portState->isHidden() ? QString() : m_portState->text();
}

void MainWindow::goToPanel(QWidget *place)
{
    auto *dock = qobject_cast<QDockWidget *>(place);
    if (!dock) {
        // The library, in the middle.
        m_library->filter()->setFocus(Qt::ShortcutFocusReason);
        return;
    }
    dock->show();
    dock->raise();
    if (QWidget *w = dock->widget()) {
        // The first thing there that takes the keyboard.
        if (QWidget *first = w->focusPolicy() != Qt::NoFocus ? w : w->nextInFocusChain(); first && w->isAncestorOf(first))
            first->setFocus(Qt::ShortcutFocusReason);
        else
            w->setFocus(Qt::ShortcutFocusReason);
    }
}

void MainWindow::showLayout(Layout layout)
{
    // The panels a task needs, shown and in front; the others out of the way.
    QList<QDockWidget *> shown;
    switch (layout) {
    case Layout::Coding:
        shown = {m_buildDock, m_problemsDock, m_simDock, m_circuitDock};
        break;
    case Layout::Debugging:
        shown = {m_simDock, m_debugDock, m_traceDock, m_pinsDock, m_problemsDock};
        break;
    case Layout::Hardware:
        shown = {m_devicesDock, m_serialDock, m_plotDock, m_buildDock};
        break;
    }
    const QList<QDockWidget *> all = {m_devicesDock, m_buildDock, m_problemsDock, m_serialDock, m_simDock, m_debugDock,
                                      m_displayDock, m_pinsDock, m_circuitDock, m_traceDock, m_plotDock};
    for (QDockWidget *d : all) {
        if (d)
            d->setVisible(shown.contains(d));
    }
    for (QDockWidget *d : std::as_const(shown)) {
        if (d)
            d->raise();
    }
}

void MainWindow::showBacktraceDialog()
{
    if (!m_backtraces) {
        m_backtraces = new BacktraceDialog(m_core, this);
        connect(m_backtraces, &BacktraceDialog::locationActivated, this, &MainWindow::showLocation);
    }
    m_backtraces->setBuild(m_project->currentProjectId(), m_project->currentBuildBoard(),
                           m_project->currentProjectTitle());
    m_backtraces->show();
    m_backtraces->raise();
    m_backtraces->activateWindow();
}

void MainWindow::find()
{
    // In the Code tab: find in the sketch.
    if (m_project->codeHasFocus()) {
        m_project->findInCode();
        return;
    }
    // In the Build dock: find in the log.
    for (QWidget *w = QApplication::focusWidget(); w; w = w->parentWidget()) {
        if (w == m_buildPanel) {
            m_buildPanel->focusFind();
            return;
        }
    }
    // The console the focus is in, if it is in one.
    for (QWidget *w = QApplication::focusWidget(); w; w = w->parentWidget()) {
        if (auto *bar = w->findChild<ConsoleBar *>(QString(), Qt::FindDirectChildrenOnly)) {
            bar->focusFind();
            return;
        }
    }
    m_library->filter()->setFocus(Qt::ShortcutFocusReason);
    m_library->filter()->selectAll();
}

QString MainWindow::shortcutsText() const
{
    // Every menu and toolbar action with a shortcut, then the consoles' own keys.
    QStringList rows;
    QStringList seen;
    const auto add = [&](const QString &keys, const QString &what) {
        if (seen.contains(keys))
            return;
        seen << keys;
        rows << u"<tr><td><b>%1</b>&nbsp;&nbsp;</td><td>%2</td></tr>"_s.arg(keys.toHtmlEscaped(), what.toHtmlEscaped());
    };
    for (QMenu *menu : menuBar()->findChildren<QMenu *>()) {
        for (QAction *a : menu->actions()) {
            if (!a->shortcut().isEmpty())
                add(a->shortcut().toString(QKeySequence::NativeText), a->text().remove(u'&'));
        }
    }
    add(QKeySequence(QKeySequence::ZoomIn).toString(QKeySequence::NativeText) + u" / "_s
            + QKeySequence(QKeySequence::ZoomOut).toString(QKeySequence::NativeText),
        tr("Bigger or smaller text in the console (also Ctrl and the mouse wheel)"));
    add(QKeySequence(Qt::CTRL | Qt::Key_0).toString(QKeySequence::NativeText), tr("The console's text at its usual size"));
    add(tr("Up / Down"), tr("In the text to send: the lines sent before"));
    add(QKeySequence(Qt::CTRL | Qt::Key_L).toString(QKeySequence::NativeText), tr("In the Code tab: go to a line"));
    add(QKeySequence(QKeySequence::Undo).toString(QKeySequence::NativeText) + u" / "_s
            + QKeySequence(QKeySequence::Redo).toString(QKeySequence::NativeText),
        tr("Undo or redo a change to the circuit"));
    add(QKeySequence(Qt::CTRL | Qt::Key_D).toString(QKeySequence::NativeText), tr("In the circuit: a copy of the selected part"));
    add(QKeySequence(QKeySequence::Copy).toString(QKeySequence::NativeText) + u" / "_s
            + QKeySequence(QKeySequence::Paste).toString(QKeySequence::NativeText),
        tr("In the circuit: copy the selected part, and paste it into this or another project's circuit"));
    add(tr("Left / Right"), tr("In the trace: move marker A to the selected pin's previous or next change (with Shift, marker B)"));
    add(QKeySequence(QKeySequence::Delete).toString(QKeySequence::NativeText), tr("In the circuit: remove the selected part"));
    add(QKeySequence(Qt::Key_Escape).toString(QKeySequence::NativeText),
        tr("Close the message above the library; in the Build dock, stop the build"));
    add(tr("Tab, arrows, Space"), tr("In the circuit: go to a part, move it, press or switch it"));
    return u"<table>%1</table>"_s.arg(rows.join(QString()));
}

void MainWindow::showBreakpointLines()
{
    const QString prefix = m_project->currentProjectId() + u".ino:"_s;
    QSet<int> lines;
    for (const QString &loc : m_debugger->breakpointLocations()) {
        if (loc.startsWith(prefix)) // "blink.ino:12", maybe "… if i == 10"
            lines.insert(loc.mid(prefix.size()).section(u' ', 0, 0).toInt());
    }
    lines.remove(0);
    m_project->codeView()->setBreakpointLines(lines);
}

void MainWindow::showShortcuts()
{
    QMessageBox box(QMessageBox::NoIcon, tr("Keyboard shortcuts"), shortcutsText(), QMessageBox::Close, this);
    box.setTextFormat(Qt::RichText);
    box.exec();
}

QStringList MainWindow::versionLines() const
{
    // Versions only: no paths, ports or project names, which can say who someone is.
    QStringList lines{u"- App: %1"_s.arg(QApplication::applicationVersion()),
                      u"- Core: %1"_s.arg(m_core->coreVersion().isEmpty() ? u"not connected"_s : m_core->coreVersion()),
                      u"- System: %1 (%2)"_s.arg(QSysInfo::prettyProductName(), QSysInfo::currentCpuArchitecture()),
                      u"- Qt: %1"_s.arg(QString::fromLatin1(qVersion()))};
    for (const QJsonValue &v : m_toolchain) {
        const QJsonObject c = v.toObject();
        QString state = c.value(u"state"_s).toString();
        if (c.contains(u"version"_s))
            state += u" "_s + c.value(u"version"_s).toString();
        lines << u"- %1: %2"_s.arg(c.value(u"id"_s).toString(), state);
    }
    return lines;
}

QUrl MainWindow::problemReportUrl() const
{
    QStringList lines{tr("What happened, and what you expected:"), QString(), QString(), u"---"_s};
    lines << versionLines();
    QUrlQuery q;
    q.addQueryItem(u"body"_s, lines.join(u'\n'));
    QUrl url(u"https://github.com/doodersrage/esp32-desktop/issues/new"_s);
    url.setQuery(q);
    return url;
}

void MainWindow::dragEnterEvent(QDragEnterEvent *event)
{
    if (!dropTargets(event->mimeData()).isEmpty())
        event->acceptProposedAction();
}

void MainWindow::dropEvent(QDropEvent *event)
{
    const QStringList targets = dropTargets(event->mimeData());
    if (targets.isEmpty())
        return;
    event->acceptProposedAction();
    for (const QString &t : targets) {
        if (t.startsWith(u"esp32bench://"_s))
            openLink(t);
        else
            m_core->openLocal(t); // the core checks it is an ESP-IDF project folder
    }
}

QStringList MainWindow::dropTargets(const QMimeData *mime)
{
    QStringList out;
    if (!mime)
        return out;
    for (const QUrl &url : mime->urls()) {
        if (url.isLocalFile())
            out << url.toLocalFile();
        else if (url.scheme() == u"esp32bench")
            out << url.toString();
    }
    if (out.isEmpty() && mime->hasText()) {
        const QString text = mime->text().trimmed();
        if (text.startsWith(u"esp32bench://"_s) && !text.contains(u'\n'))
            out << text;
    }
    return out;
}

void MainWindow::showSetup()
{
    m_core->requestToolchainStatus();
    m_setup->show();
    m_setup->raise();
}

bool MainWindow::confirmFlash(const QString &port, const QJsonObject &plan)
{
    QMessageBox box(QMessageBox::Warning, tr("Flash a downloaded project?"),
                    tr("“%1” for %2 came from esp32bench.dev. Flashing it to %3 replaces the firmware "
                       "on that board and runs this code on it.")
                        .arg(plan.value(u"project"_s).toString(), plan.value(u"board"_s).toString(), port),
                    QMessageBox::Cancel, this);
    box.setInformativeText(tr("%1 will be written. Saved settings past the image are kept.")
                               .arg(QLocale().formattedDataSize(plan.value(u"bytes"_s).toInteger())));
    box.setDetailedText(tr("Image SHA-256: %1").arg(plan.value(u"imageSha256"_s).toString()));
    QPushButton *flash = box.addButton(tr("Flash"), QMessageBox::AcceptRole);
    box.setDefaultButton(QMessageBox::Cancel);
    box.exec();
    return box.clickedButton() == flash;
}

bool MainWindow::confirmErase(const QString &port)
{
    QMessageBox box(QMessageBox::Warning, tr("Erase the whole flash?"),
                    tr("Everything on the board at %1 is erased: firmware, saved settings and files. "
                       "This cannot be undone.")
                        .arg(port),
                    QMessageBox::Cancel, this);
    QPushButton *erase = box.addButton(tr("Erase"), QMessageBox::DestructiveRole);
    box.setDefaultButton(QMessageBox::Cancel);
    box.exec();
    return box.clickedButton() == erase;
}

void MainWindow::openSerialPort(const QString &path)
{
    m_serialDock->show();
    m_serialDock->raise();
    m_serial->openPort(path);
}

void MainWindow::inspectPort(const QString &port)
{
    freePort(port);
    statusBar()->showMessage(tr("Reading the board on %1…").arg(port));
    m_core->inspectBoard(port);
}

void MainWindow::erasePort(const QString &port)
{
    if (confirmErase(port)) {
        freePort(port);
        statusBar()->showMessage(tr("Erasing the board on %1…").arg(port));
        m_core->eraseBoard(port);
    }
}

void MainWindow::freePort(const QString &port)
{
    // Board operations need the port to themselves; the monitor comes back afterwards.
    if (m_core->isSerialOpen()) {
        m_reopenSerial = port;
        m_core->closeSerial();
    }
}
