#include "MainWindow.h"

#include "BoardInfoDialog.h"
#include "BuildPanel.h"
#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "LibraryPanel.h"
#include "MessageBar.h"
#include "ProjectView.h"
#include "SerialMonitor.h"
#include "SetupDialog.h"
#include "SimulatorPanel.h"
#include "DebugPanel.h"
#include "DisplayPanel.h"
#include "PinsPanel.h"
#include "TracePanel.h"
#include "CircuitPanel.h"

#include <algorithm>

#include <QAction>
#include <QApplication>
#include <QComboBox>
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
    // Errors stay above the library and project until closed or a new attempt starts.
    auto *centre = new QWidget(this);
    m_messages = new MessageBar(centre);
    auto *centreLayout = new QVBoxLayout(centre);
    centreLayout->setContentsMargins(0, 0, 0, 0);
    centreLayout->addWidget(m_messages);
    centreLayout->addWidget(m_split, 1);
    setCentralWidget(centre);
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
    connect(core, &CoreConnection::buildFinished, this, [this](bool ok, const QString &error) {
        if (!ok)
            m_messages->showError(tr("The build failed: %1\nThe Build panel below has the whole log.").arg(error));
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

    m_serialDock = new QDockWidget(tr("Serial monitor"), this);
    m_serialDock->setObjectName(u"serialDock"_s);
    m_serial = new SerialMonitor(core, m_serialDock);
    m_serialDock->setWidget(m_serial);
    addDockWidget(Qt::BottomDockWidgetArea, m_serialDock);
    tabifyDockWidget(m_buildDock, m_serialDock);

    m_simDock = new QDockWidget(tr("Simulator"), this);
    m_simDock->setObjectName(u"simDock"_s);
    m_sim = new SimulatorPanel(core, m_simDock);
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
    m_circuitDock->setWidget(m_circuit);
    addDockWidget(Qt::RightDockWidgetArea, m_circuitDock);
    // One tab group on the right, so the canvas gets the full height: Circuit, Pins
    // and Display. Each is raised when it has something to show.
    tabifyDockWidget(m_circuitDock, m_pinsDock);
    tabifyDockWidget(m_circuitDock, m_displayDock);
    m_circuitDock->raise();
    connect(m_project, &ProjectView::boardSelected, m_circuit, &CircuitPanel::showBoard);

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
                    m_serial->openPort(m_lastBoardPort);
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
        if (method == u"link.open")
            m_messages->showError(tr("Could not open the link: %1").arg(message));
    });
    connect(core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        if (s == CoreConnection::State::Connected) {
            // A link names what to show; otherwise reopen what was shown last time.
            if (m_pendingLinks.isEmpty() && !m_reopened) {
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
    statusBar()->addPermanentWidget(m_startCore);

    // At first only the simulator's console is open below; the build log and the serial
    // monitor open when a build starts or a port is opened.
    m_buildDock->hide();
    m_serialDock->hide();
    // The toolbar picks the port and its Board menu has what the Devices panel's
    // context menu has; the panel, with every port's details, is in the View menu.
    m_devicesDock->hide();

    buildToolBar();
    buildMenus();
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
    connect(m_core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        if (s == CoreConnection::State::Connected)
            m_core->requestToolchainStatus();
    });
    updateToolBar();
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
    m_boardMenu->setEnabled(m_buildPanel->portPicker()->currentIndex() >= 0
                            && m_core->state() == CoreConnection::State::Connected && !m_core->isBoardBusy());
    m_busyAction->setVisible(m_core->isBuilding() || m_core->isBoardBusy());

    const QString id = m_project->currentProjectId();
    const bool built = !id.isEmpty() && m_buildPanel->builtProject() == id
                       && m_buildPanel->builtBoard() == m_project->currentBuildBoard();
    QString next;
    if (m_core->state() != CoreConnection::State::Connected)
        next.clear();
    else if (m_core->isBuilding())
        next = tr("Building…");
    else if (m_core->isBoardBusy())
        next = tr("Working on the board…");
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
    QAction *quit = file->addAction(QIcon::fromTheme(u"application-exit"_s), tr("&Quit"));
    quit->setShortcut(QKeySequence::Quit);
    connect(quit, &QAction::triggered, this, &QWidget::close);

    // The toolbar's steps, where their shortcuts show.
    QMenu *project = menuBar()->addMenu(tr("&Project"));
    project->addAction(m_project->buildAction());
    project->addAction(m_buildPanel->simulateAction());
    project->addAction(m_buildPanel->debugAction());
    project->addAction(m_stopSim);
    project->addSeparator();
    project->addAction(m_buildPanel->flashAction());

    QMenu *view = menuBar()->addMenu(tr("&View"));
    view->addAction(m_devicesDock->toggleViewAction());
    view->addAction(m_buildDock->toggleViewAction());
    view->addAction(m_serialDock->toggleViewAction());
    view->addAction(m_simDock->toggleViewAction());
    view->addAction(m_traceDock->toggleViewAction());
    view->addAction(m_debugDock->toggleViewAction());
    view->addAction(m_displayDock->toggleViewAction());
    view->addAction(m_pinsDock->toggleViewAction());
    view->addAction(m_circuitDock->toggleViewAction());
    view->addSeparator();
    QAction *reset = view->addAction(QIcon::fromTheme(u"view-restore"_s), tr("&Reset layout"));
    reset->setToolTip(tr("Put the panels back where they were at first"));
    connect(reset, &QAction::triggered, this, &MainWindow::resetLayout);

    QMenu *tools = menuBar()->addMenu(tr("&Tools"));
    QAction *setup = tools->addAction(QIcon::fromTheme(u"configure"_s), tr("&Toolchain setup…"));
    connect(setup, &QAction::triggered, this, &MainWindow::showSetup);

    QMenu *help = menuBar()->addMenu(tr("&Help"));
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
