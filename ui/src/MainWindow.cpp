#include "MainWindow.h"

#include "CoreConnection.h"
#include "DevicesPanel.h"

#include <QAction>
#include <QApplication>
#include <QCloseEvent>
#include <QDockWidget>
#include <QIcon>
#include <QLabel>
#include <QMenuBar>
#include <QMessageBox>
#include <QPushButton>
#include <QSettings>
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

    // The project library goes in the centre in a later step.
    auto *placeholder = new QLabel(tr("Projects will appear here."), this);
    placeholder->setAlignment(Qt::AlignCenter);
    placeholder->setEnabled(false);
    setCentralWidget(placeholder);

    m_devicesDock->setObjectName(u"devicesDock"_s); // needed for saveState()
    m_devices = new DevicesPanel(core, m_devicesDock);
    m_devicesDock->setWidget(m_devices);
    addDockWidget(Qt::LeftDockWidgetArea, m_devicesDock);

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
        resize(1100, 700);
    const QByteArray state = settings.value(u"mainWindow/state"_s).toByteArray();
    if (state.isEmpty() || !restoreState(state))
        resizeDocks({m_devicesDock}, {440}, Qt::Horizontal); // first start
}

void MainWindow::saveLayout()
{
    QSettings settings;
    settings.setValue(u"mainWindow/geometry"_s, saveGeometry());
    settings.setValue(u"mainWindow/state"_s, saveState());
}

void MainWindow::closeEvent(QCloseEvent *event)
{
    saveLayout();
    QMainWindow::closeEvent(event);
}
