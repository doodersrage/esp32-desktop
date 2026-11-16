#include "MainWindow.h"

#include "BoardInfoDialog.h"
#include "BuildPanel.h"
#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "LibraryPanel.h"
#include "ProjectView.h"
#include "SerialMonitor.h"
#include "SetupDialog.h"
#include "SimulatorPanel.h"
#include "DebugPanel.h"
#include "DisplayPanel.h"

#include <algorithm>

#include <QAction>
#include <QApplication>
#include <QCloseEvent>
#include <QDockWidget>
#include <QIcon>
#include <QJsonArray>
#include <QLocale>
#include <QLabel>
#include <QMenuBar>
#include <QMessageBox>
#include <memory>
#include <QPushButton>
#include <QSettings>
#include <QSplitter>
#include <QStatusBar>

using namespace Qt::StringLiterals;

MainWindow::MainWindow(CoreConnection *core, QWidget *parent)
    : QMainWindow(parent)
    , m_core(core)
    , m_devices(nullptr)
    , m_devicesDock(new QDockWidget(tr("Devices"), this))
    , m_connection(new QLabel(this))
    , m_startCore(new QPushButton(tr("Start core"), this))
{
    setWindowTitle(tr("ESP32 Bench"));
    setWindowIcon(QIcon::fromTheme(u"applications-electronics"_s));

    m_split = new QSplitter(Qt::Horizontal, this);
    m_split->setObjectName(u"librarySplit"_s);
    m_library = new LibraryPanel(core, m_split);
    m_project = new ProjectView(core, m_split);
    m_split->addWidget(m_library);
    m_split->addWidget(m_project);
    m_split->setStretchFactor(0, 1);
    m_split->setStretchFactor(1, 1);
    connect(m_library, &LibraryPanel::projectSelected, m_project, &ProjectView::showProject);
    setCentralWidget(m_split);

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

    m_debugDock = new QDockWidget(tr("Debugger"), this);
    m_debugDock->setObjectName(u"debugDock"_s);
    m_debugger = new DebugPanel(core, m_debugDock);
    m_debugDock->setWidget(m_debugger);
    // Beside the simulator's console, not tabbed over it: both are needed at once.
    splitDockWidget(m_simDock, m_debugDock, Qt::Horizontal);
    m_debugDock->hide(); // until the first debug session
    connect(core, &CoreConnection::debugStarted, this, [this] {
        m_simDock->show();
        m_debugDock->show();
        m_debugDock->raise();
    });
    connect(m_devices, &DevicesPanel::portActivated, this, [this](const QString &path) {
        m_serialDock->show();
        m_serialDock->raise();
        m_serial->openPort(path);
    });
    connect(core, &CoreConnection::buildConsentRequired, this,
            [this](const QString &project, const QString &board, const QString &token, const QJsonObject &plan) {
                if (confirmBuild(project, board, plan))
                    m_core->startBuild(project, board, token);
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
    connect(m_devices, &DevicesPanel::inspectRequested, this, [this](const QString &port) {
        freePort(port);
        statusBar()->showMessage(tr("Reading the board on %1…").arg(port));
        m_core->inspectBoard(port);
    });
    connect(m_devices, &DevicesPanel::eraseRequested, this, [this](const QString &port) {
        if (confirmErase(port)) {
            freePort(port);
            statusBar()->showMessage(tr("Erasing the board on %1…").arg(port));
            m_core->eraseBoard(port);
        }
    });
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
                    statusBar()->showMessage(tr("Board operation failed: %1").arg(error), 10000);
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
    connect(core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
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
    statusBar()->addWidget(m_connection, 1);
    statusBar()->addPermanentWidget(m_startCore);

    buildMenus();
    connect(m_core, &CoreConnection::stateChanged, this, &MainWindow::showConnectionState);
    showConnectionState();
    restoreLayout();
}

QString MainWindow::connectionText() const
{
    return m_connection->text();
}

void MainWindow::buildMenus()
{
    QMenu *file = menuBar()->addMenu(tr("&File"));
    QAction *quit = file->addAction(QIcon::fromTheme(u"application-exit"_s), tr("&Quit"));
    quit->setShortcut(QKeySequence::Quit);
    connect(quit, &QAction::triggered, this, &QWidget::close);

    QMenu *view = menuBar()->addMenu(tr("&View"));
    view->addAction(m_devicesDock->toggleViewAction());
    view->addAction(m_buildDock->toggleViewAction());
    view->addAction(m_serialDock->toggleViewAction());
    view->addAction(m_simDock->toggleViewAction());
    view->addAction(m_debugDock->toggleViewAction());
    view->addAction(m_displayDock->toggleViewAction());

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
        m_connection->setText(tr("Connected to core %1").arg(m_core->coreVersion()));
        break;
    case S::Connecting:
        m_connection->setText(tr("Connecting to the core…"));
        break;
    case S::Incompatible:
        m_connection->setText(tr("Incompatible core: %1").arg(m_core->lastError()));
        break;
    case S::Disconnected:
        m_connection->setText(tr("The core is not running. Retrying…"));
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
    if (state.isEmpty() || !restoreState(state))
        resizeDocks({m_devicesDock}, {340}, Qt::Horizontal); // first start
    m_split->restoreState(settings.value(u"mainWindow/librarySplit"_s).toByteArray());
}

void MainWindow::saveLayout()
{
    QSettings settings;
    settings.setValue(u"mainWindow/geometry"_s, saveGeometry());
    settings.setValue(u"mainWindow/state"_s, saveState());
    settings.setValue(u"mainWindow/librarySplit"_s, m_split->saveState());
}

void MainWindow::closeEvent(QCloseEvent *event)
{
    saveLayout();
    QMainWindow::closeEvent(event);
}

bool MainWindow::confirmBuild(const QString &project, const QString &board, const QJsonObject &plan)
{
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

void MainWindow::freePort(const QString &port)
{
    // Board operations need the port to themselves; the monitor comes back afterwards.
    if (m_core->isSerialOpen()) {
        m_reopenSerial = port;
        m_core->closeSerial();
    }
}
