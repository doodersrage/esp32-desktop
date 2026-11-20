#include "BuildPanel.h"

#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "Labels.h"

#include <QAction>
#include <QComboBox>
#include <QFontDatabase>
#include <QJsonArray>
#include <QProgressBar>
#include <QHBoxLayout>
#include <QIcon>
#include <QJsonObject>
#include <QKeySequence>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
// Keep the log bounded; arduino-cli can print a lot with many libraries.
constexpr int MaxLogLines = 20000;
}

BuildPanel::BuildPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(tr("No build yet."), this))
    , m_cancel(new QPushButton(QIcon::fromTheme(u"process-stop"_s), tr("Cancel"), this))
    , m_log(new QPlainTextEdit(this))
    , m_port(new QComboBox(this))
    , m_flash(new QAction(QIcon::fromTheme(u"media-flash"_s, QIcon::fromTheme(u"document-send"_s)),
                          tr("Flash"), this))
    , m_simulate(new QAction(QIcon::fromTheme(u"media-playback-start"_s), tr("Run in simulator"), this))
    , m_debug(new QAction(QIcon::fromTheme(u"debug-run"_s, QIcon::fromTheme(u"tools-report-bug"_s)), tr("Debug"),
                          this))
    , m_progress(new QProgressBar(this))
{
    m_simulate->setToolTip(tr("Run the selected project in the simulator, building it first if needed "
                              "(ESP32, ESP32-S3 and ESP32-C3 boards)"));
    m_simulate->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_R));
    connect(m_simulate, &QAction::triggered, this, [this] { act(Then::Run); });
    m_debug->setToolTip(tr("Run the selected project in the simulator under GDB, paused before it starts, "
                           "building it first if needed"));
    m_debug->setShortcut(QKeySequence(Qt::Key_F5));
    connect(m_debug, &QAction::triggered, this, [this] { act(Then::Debug); });
    m_port->setToolTip(tr("Board to flash"));
    m_port->setMinimumContentsLength(16);
    m_port->setSizeAdjustPolicy(QComboBox::AdjustToContents);
    m_flash->setToolTip(tr("Write the selected project to the board on the selected port, building it first "
                           "if needed"));
    m_flash->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_U));
    m_port->setPlaceholderText(tr("No board"));
    connect(m_port, &QComboBox::currentIndexChanged, this, &BuildPanel::updateFlashButton);
    connect(m_flash, &QAction::triggered, this, [this] { act(Then::Flash); });
    m_progress->setVisible(false);
    m_progress->setTextVisible(true);
    m_status->setWordWrap(true);
    m_status->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_cancel->setEnabled(false);
    connect(m_cancel, &QPushButton::clicked, m_core, &CoreConnection::cancelBuild);

    m_log->setReadOnly(true);
    m_log->setLineWrapMode(QPlainTextEdit::NoWrap);
    m_log->setMaximumBlockCount(MaxLogLines);
    m_log->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_cancel);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_progress);
    layout->addWidget(m_log, 1);

    connect(m_core, &CoreConnection::buildStarted, this, &BuildPanel::started);
    connect(m_core, &CoreConnection::buildLog, this, &BuildPanel::appendLine);
    connect(m_core, &CoreConnection::buildFinished, this, &BuildPanel::finished);
    connect(m_core, &CoreConnection::devicesListed, this, &BuildPanel::setPorts);
    connect(m_core, &CoreConnection::stateChanged, this, &BuildPanel::updateFlashButton);
    connect(m_core, &CoreConnection::boardStarted, this, [this](const QString &op, const QString &port) {
        if (op != u"flash")
            return;
        m_progress->setRange(0, 0); // busy until the first byte count
        m_progress->setVisible(true);
        m_status->setText(tr("Flashing %1 to %2…").arg(m_builtProject, port));
        updateFlashButton();
    });
    connect(m_core, &CoreConnection::boardProgress, this, [this](const QString &stage, qint64 written, qint64 total) {
        if (!m_progress->isVisible())
            return;
        if (stage == u"writing" && total > 0) {
            m_progress->setRange(0, 1000);
            m_progress->setValue(int(written * 1000 / total));
            m_progress->setFormat(tr("Writing %p%"));
        } else if (stage == u"verifying") {
            m_progress->setFormat(tr("Verifying"));
        } else if (stage == u"connecting") {
            m_progress->setFormat(tr("Connecting to the bootloader"));
        }
    });
    connect(m_core, &CoreConnection::boardFinished, this,
            [this](const QString &op, bool ok, const QString &error, const QJsonObject &result) {
                if (op != u"flash")
                    return;
                m_progress->setVisible(false);
                m_status->setText(ok ? tr("Flashed %1 bytes. The board is running the new firmware.")
                                           .arg(result.value(u"written"_s).toInteger())
                                     : tr("Flashing failed: %1").arg(error));
                updateFlashButton();
            });
    updateFlashButton();
}

QString BuildPanel::statusText() const
{
    return m_status->text();
}

QString BuildPanel::logText() const
{
    return m_log->toPlainText();
}

void BuildPanel::started(const QJsonObject &plan)
{
    m_log->clear();
    m_pendingProject = plan.value(u"project"_s).toString();
    m_pendingBoard = plan.value(u"board"_s).toString();
    m_status->setText(tr("Building %1 for %2…")
                          .arg(plan.value(u"project"_s).toString(), plan.value(u"board"_s).toString()));
    m_cancel->setEnabled(true);
}

void BuildPanel::appendLine(const QString &, const QString &line)
{
    m_log->appendPlainText(line);
}

void BuildPanel::finished(bool ok, const QString &error, const QString &image)
{
    m_cancel->setEnabled(false);
    m_status->setText(ok ? tr("Build succeeded. Image: %1").arg(image)
                         : tr("Build failed: %1").arg(error));
    if (ok) {
        m_builtProject = m_pendingProject;
        m_builtBoard = m_pendingBoard;
    }
    // What was asked for before the build (run, debug, flash) follows a build of the
    // project it was asked for; a failed build drops it.
    const Then then = std::exchange(m_then, Then::Nothing);
    updateFlashButton();
    if (ok && then != Then::Nothing && targetBuilt())
        act(then);
}

void BuildPanel::setTarget(const QString &project, const QString &board, bool canBuild)
{
    if (project != m_targetProject || board != m_targetBoard)
        m_then = Then::Nothing; // a different project now: forget what the last one wanted
    m_targetProject = project;
    m_targetBoard = board;
    m_canBuild = canBuild;
    updateFlashButton();
}

void BuildPanel::cancelPending()
{
    m_then = Then::Nothing;
}

QString BuildPanel::targetProject() const
{
    return m_targetProject.isEmpty() ? m_builtProject : m_targetProject;
}

QString BuildPanel::targetBoard() const
{
    return m_targetProject.isEmpty() ? m_builtBoard : m_targetBoard;
}

bool BuildPanel::targetBuilt() const
{
    return !m_builtProject.isEmpty() && targetProject() == m_builtProject && targetBoard() == m_builtBoard;
}

void BuildPanel::act(Then what)
{
    if (!targetBuilt()) {
        // Build the project shown first; the build's end does the rest (finished()).
        m_then = what;
        emit buildRequested();
        return;
    }
    switch (what) {
    case Then::Run:
    case Then::Debug:
        if (m_core->isSimRunning())
            m_core->stopSim();
        m_core->startSim(targetProject(), targetBoard(), what == Then::Debug);
        break;
    case Then::Flash:
        emit flashRequested(m_port->currentData().toString(), targetProject(), targetBoard());
        break;
    case Then::Nothing:
        break;
    }
}

void BuildPanel::setPorts(const QJsonArray &ports)
{
    const QString current = m_port->currentData().toString();
    QStringList paths;
    m_port->clear();
    for (const QJsonValue &v : ports) {
        const QJsonObject p = v.toObject();
        const QString path = p.value(u"path"_s).toString();
        paths << path;
        // What is on the port, then its short name: "ESP32-S3 – ttyACM0".
        const QJsonArray chips = p.value(u"chips"_s).toArray();
        const QString bridge = p.value(u"bridge"_s).toString();
        const QString what = chips.size() == 1 ? Labels::chipName(chips.at(0).toString())
                             : bridge.isEmpty() ? QString()
                                                : DevicesPanel::bridgeName(bridge);
        const QString name = path.section(u'/', -1);
        m_port->addItem(what.isEmpty() ? name : tr("%1 – %2").arg(what, name), path);
        if (p.value(u"access"_s).toString() == u"denied") {
            m_port->setItemIcon(m_port->count() - 1, QIcon::fromTheme(u"dialog-warning"_s));
            m_port->setItemData(m_port->count() - 1, u"%1\n%2"_s.arg(path, DevicesPanel::accessHint()),
                                Qt::ToolTipRole);
        } else {
            m_port->setItemData(m_port->count() - 1, path, Qt::ToolTipRole);
        }
    }
    // A board just plugged in is the one to flash; otherwise keep the port that was
    // chosen, else take the first. (With a placeholder text the combo selects nothing
    // by itself.)
    QString pick = current;
    for (const QString &path : std::as_const(paths)) {
        if (!m_knownPorts.contains(path) && !m_knownPorts.isEmpty())
            pick = path;
    }
    m_knownPorts = paths;
    if (m_knownPorts.isEmpty())
        m_knownPorts << QString(); // seen an empty list: the next board is new
    const int i = m_port->findData(pick);
    m_port->setCurrentIndex(i >= 0 ? i : (m_port->count() > 0 ? 0 : -1));
    updateFlashButton();
}

void BuildPanel::updateFlashButton()
{
    const bool connected = m_core->state() == CoreConnection::State::Connected;
    // Something to act on: built already, or buildable now.
    const bool ready = connected && !targetProject().isEmpty() && !m_core->isBuilding()
                       && (targetBuilt() || (!m_targetProject.isEmpty() && m_canBuild));
    m_simulate->setEnabled(ready);
    m_debug->setEnabled(ready);
    m_flash->setEnabled(ready && m_port->currentIndex() >= 0 && !m_core->isBoardBusy());
    emit actionsChanged();
}
