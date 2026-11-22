#include "BuildPanel.h"

#include <QLineEdit>
#include <QTextDocument>
#include <QToolButton>

#include "ConsoleView.h"
#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "Labels.h"

#include <QAction>
#include <QCheckBox>
#include <QClipboard>
#include <QGuiApplication>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QComboBox>
#include <QTextBlock>
#include <QFileInfo>
#include <QFontDatabase>
#include <QJsonArray>
#include <QProgressBar>
#include <QHBoxLayout>
#include <QIcon>
#include <QJsonObject>
#include <QKeySequence>
#include <QLabel>
#include <QLocale>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
// What an error or warning line points at (build.log's location); its block owns it.
class LocationData : public QTextBlockUserData {
public:
    explicit LocationData(QJsonObject l)
        : location(std::move(l))
    {
    }
    QJsonObject location;
};
}

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
    , m_problems(new QLabel(this))
    , m_problemsOnly(new QCheckBox(tr("Problems only"), this))
    , m_port(new QComboBox(this))
    , m_flash(new QAction(QIcon::fromTheme(u"media-flash"_s, QIcon::fromTheme(u"document-send"_s)),
                          tr("Flash"), this))
    , m_simulate(new QAction(QIcon::fromTheme(u"media-playback-start"_s), tr("Run in simulator"), this))
    , m_debug(new QAction(QIcon::fromTheme(u"debug-run"_s, QIcon::fromTheme(u"tools-report-bug"_s)), tr("Debug"),
                          this))
    , m_progress(new QProgressBar(this))
{
    m_log->setAccessibleName(tr("Build log"));
    m_log->setToolTip(tr("Double-click an error or warning, or press Enter on it, to see the line it points at"));
    m_port->setAccessibleName(tr("Board port"));
    m_simulate->setToolTip(tr("Run the selected project in the simulator, building it first if needed "
                              "(ESP32, ESP32-S3 and ESP32-C3 boards)"));
    m_simulate->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_R));
    connect(m_simulate, &QAction::triggered, this, [this] { act(Then::Run); });
    m_debug->setToolTip(tr("Run the selected project in the simulator under GDB, paused before it starts, "
                           "building it first if needed"));
    m_debug->setShortcut(QKeySequence(Qt::Key_F5));
    connect(m_debug, &QAction::triggered, this, [this] { act(Then::Debug); });
    m_nextProblem = new QAction(QIcon::fromTheme(u"go-down"_s), tr("Next problem"), this);
    m_nextProblem->setShortcut(QKeySequence(Qt::Key_F8));
    m_nextProblem->setToolTip(tr("Show the line of the build's next error or warning"));
    m_previousProblem = new QAction(QIcon::fromTheme(u"go-up"_s), tr("Previous problem"), this);
    m_previousProblem->setShortcut(QKeySequence(Qt::SHIFT | Qt::Key_F8));
    m_previousProblem->setToolTip(tr("Show the line of the build's previous error or warning"));
    for (QAction *a : {m_nextProblem, m_previousProblem})
        a->setEnabled(false);
    connect(m_nextProblem, &QAction::triggered, this, [this] { goToProblem(true); });
    connect(m_previousProblem, &QAction::triggered, this, [this] { goToProblem(false); });
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

    m_problemsOnly->setToolTip(tr("Show only the build's errors and warnings"));
    m_copyProblems = new QPushButton(QIcon::fromTheme(u"edit-copy"_s), tr("Copy problems"), this);
    m_copyProblems->setToolTip(tr("Copy the build's error and warning lines, to paste into a forum post or an issue"));
    m_copyProblems->hide();
    connect(m_copyProblems, &QPushButton::clicked, this,
            [this] { QGuiApplication::clipboard()->setText(problemLines()); });
    m_log->viewport()->installEventFilter(this);
    m_log->installEventFilter(this);
    m_problems->hide();
    connect(m_problemsOnly, &QCheckBox::toggled, this, [this](bool on) {
        for (QTextBlock b = m_log->document()->begin(); b.isValid(); b = b.next())
            b.setVisible(!on || b.userState() == OutputLines::Error || b.userState() == OutputLines::Warning);
        m_log->document()->markContentsDirty(0, m_log->document()->characterCount());
        m_log->viewport()->update();
    });
    m_log->setReadOnly(true);
    m_log->setLineWrapMode(QPlainTextEdit::NoWrap);
    m_log->setMaximumBlockCount(MaxLogLines);
    m_log->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_problems);
    top->addWidget(m_problemsOnly);
    top->addWidget(m_copyProblems);
    m_forum = new QToolButton(this);
    m_forum->setIcon(QIcon::fromTheme(u"edit-copy"_s));
    m_forum->setText(tr("Copy for a forum post"));
    m_forum->setAccessibleName(m_forum->text());
    m_forum->setToolTip(tr("Copy the build's errors and warnings (or the last 200 lines of the log) as a code block "
                           "for a forum post or an issue, with the app's version, the project and the board"));
    connect(m_forum, &QToolButton::clicked, this, [this] { QGuiApplication::clipboard()->setText(forumText()); });
    top->addWidget(m_forum);
    // Find in the log (Ctrl+F in this dock): Enter for the next, Shift+Enter the last.
    m_find = new QLineEdit(this);
    m_find->setPlaceholderText(tr("Find in the log"));
    m_find->setAccessibleName(tr("Find in the log"));
    m_find->setClearButtonEnabled(true);
    m_find->setMaximumWidth(220);
    connect(m_find, &QLineEdit::returnPressed, this,
            [this] { findText(m_find->text(), QGuiApplication::keyboardModifiers() & Qt::ShiftModifier); });
    top->addWidget(m_find);
    top->addWidget(m_cancel);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_progress);
    layout->addWidget(m_log, 1);

    connect(m_core, &CoreConnection::buildStarted, this, &BuildPanel::started);
    connect(m_core, &CoreConnection::buildSize, this, [this](const QJsonObject &size) { m_size = size; });
    connect(m_core, &CoreConnection::buildTime, this, [this](double seconds) { m_seconds = seconds; });
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
        m_flashClock.invalidate();
        updateFlashButton();
    });
    connect(m_core, &CoreConnection::boardProgress, this, [this](const QString &stage, qint64 written, qint64 total) {
        if (!m_progress->isVisible())
            return;
        if (stage == u"writing" && total > 0) {
            m_progress->setRange(0, 1000);
            m_progress->setValue(int(written * 1000 / total));
            m_progress->setFormat(flashProgressText(written, total));
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
    m_nextProblem->setEnabled(false);
    m_previousProblem->setEnabled(false);
    m_errors = m_warnings = 0;
    showProblemCount();
    m_pendingProject = plan.value(u"project"_s).toString();
    m_pendingBoard = plan.value(u"board"_s).toString();
    m_status->setText(tr("Building %1 for %2…")
                          .arg(plan.value(u"project"_s).toString(), plan.value(u"board"_s).toString()));
    m_cancel->setEnabled(true);
}

void BuildPanel::appendLine(const QString &, const QString &line, const QString &level,
                            const QJsonObject &location)
{
    m_log->appendPlainText(line);
    const int state = OutputLines::state(level);
    QTextBlock b = m_log->document()->lastBlock();
    OutputLines::mark(b, state, {}, m_log->palette());
    if (!location.isEmpty()) {
        b.setUserData(new LocationData(location)); // the block owns it
        m_nextProblem->setEnabled(true);
        m_previousProblem->setEnabled(true);
    }
    if (state == OutputLines::Error)
        ++m_errors;
    else if (state == OutputLines::Warning)
        ++m_warnings;
    if (m_problemsOnly->isChecked() && state != OutputLines::Error && state != OutputLines::Warning) {
        b.setVisible(false);
        m_log->document()->markContentsDirty(b.position(), b.length());
    }
    showProblemCount();
}

bool BuildPanel::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == m_log->viewport() && event->type() == QEvent::MouseButtonDblClick) {
        const auto *mouse = static_cast<QMouseEvent *>(event);
        if (activateLine(m_log->cursorForPosition(mouse->position().toPoint()).blockNumber()))
            return true;
    } else if (watched == m_log && event->type() == QEvent::KeyPress) {
        const int key = static_cast<QKeyEvent *>(event)->key();
        if ((key == Qt::Key_Return || key == Qt::Key_Enter) && activateLine(m_log->textCursor().blockNumber()))
            return true;
    }
    return QWidget::eventFilter(watched, event);
}

bool BuildPanel::activateLine(int blockNumber)
{
    const QTextBlock b = m_log->document()->findBlockByNumber(blockNumber);
    const auto *d = static_cast<const LocationData *>(b.userData());
    if (!d)
        return false;
    emit locationActivated(d->location);
    return true;
}

bool BuildPanel::findText(const QString &text, bool backward)
{
    if (text.isEmpty())
        return false;
    const QTextDocument::FindFlags flags = backward ? QTextDocument::FindBackward : QTextDocument::FindFlags();
    bool found = m_log->find(text, flags);
    if (!found) {
        // Round from the other end.
        QTextCursor c(m_log->document());
        c.movePosition(backward ? QTextCursor::End : QTextCursor::Start);
        m_log->setTextCursor(c);
        found = m_log->find(text, flags);
    }
    m_find->setToolTip(found ? QString() : tr("Not found"));
    return found;
}

void BuildPanel::focusFind()
{
    m_find->setFocus(Qt::ShortcutFocusReason);
    m_find->selectAll();
}

bool BuildPanel::goToProblem(bool forward)
{
    QTextDocument *doc = m_log->document();
    const int count = doc->blockCount();
    const int from = m_log->textCursor().hasSelection() || m_log->textCursor().blockNumber() > 0
        ? m_log->textCursor().blockNumber()
        : (forward ? -1 : count);
    for (int step = 1; step <= count; ++step) {
        // Round from the other end.
        const int n = ((forward ? from + step : from - step) % count + count) % count;
        const QTextBlock b = doc->findBlockByNumber(n);
        if (!b.userData())
            continue;
        QTextCursor c(b);
        c.movePosition(QTextCursor::EndOfBlock, QTextCursor::KeepAnchor);
        m_log->setTextCursor(c);
        m_log->ensureCursorVisible();
        return activateLine(n);
    }
    return false;
}

bool BuildPanel::cancelRunning()
{
    if (!m_cancel->isEnabled())
        return false;
    m_cancel->click();
    return true;
}

QString BuildPanel::forumText() const
{
    QString text = problemLines();
    if (text.isEmpty()) {
        QStringList lines;
        for (QTextBlock b = m_log->document()->begin(); b.isValid(); b = b.next())
            lines << b.text();
        text = lines.mid(std::max<qsizetype>(0, lines.size() - 200)).join(u'\n');
    }
    const QString context = m_pendingProject.isEmpty()
        ? QString()
        : tr("building %1 for %2").arg(m_pendingProject, m_pendingBoard);
    return Labels::forumPost(context, text);
}

QString BuildPanel::problemLines() const
{
    QStringList lines;
    for (QTextBlock b = m_log->document()->begin(); b.isValid(); b = b.next()) {
        if (b.userState() == OutputLines::Error || b.userState() == OutputLines::Warning)
            lines << b.text();
    }
    return lines.join(u'\n');
}

void BuildPanel::showProblemCount()
{
    QStringList parts;
    if (m_errors > 0)
        parts << tr("Errors: %1").arg(m_errors);
    if (m_warnings > 0)
        parts << tr("Warnings: %1").arg(m_warnings);
    m_problems->setText(parts.join(u"  "_s));
    m_problems->setVisible(!parts.isEmpty());
    m_copyProblems->setVisible(!parts.isEmpty());
}

void BuildPanel::finished(bool ok, const QString &error, const QString &image)
{
    m_cancel->setEnabled(false);
    // The image's name; where it is, in the tooltip.
    m_status->setText(ok ? tr("Build succeeded: %1.").arg(QFileInfo(image).fileName())
                         : tr("Build failed: %1").arg(error));
    m_status->setToolTip(ok ? image : QString());
    if (ok && !m_size.isEmpty()) {
        // How much of the board it takes; nearly full is worth a warning.
        const double flash = m_size.value(u"flash"_s).toDouble() / std::max(1.0, m_size.value(u"flashMax"_s).toDouble());
        const double ram = m_size.value(u"ram"_s).toDouble() / std::max(1.0, m_size.value(u"ramMax"_s).toDouble());
        QString size = m_size.contains(u"ram"_s)
            ? tr("It uses %1% of the flash for programs and %2% of the RAM.").arg(qRound(flash * 100)).arg(qRound(ram * 100))
            : tr("It uses %1% of the flash for programs.").arg(qRound(flash * 100));
        if (flash >= 0.9 || ram >= 0.9)
            size += u" "_s + tr("That is nearly full.");
        m_status->setText(m_status->text() + u" "_s + size);
        const QLocale loc;
        m_status->setToolTip(tr("%1\nProgram: %2 of %3\nGlobal variables: %4 of %5")
                                 .arg(image, loc.formattedDataSize(m_size.value(u"flash"_s).toInteger()),
                                      loc.formattedDataSize(m_size.value(u"flashMax"_s).toInteger()),
                                      loc.formattedDataSize(m_size.value(u"ram"_s).toInteger()),
                                      loc.formattedDataSize(m_size.value(u"ramMax"_s).toInteger())));
    }
    m_size = {};
    // How long it took.
    if (m_seconds >= 0)
        m_status->setText(m_status->text() + u" "_s + tr("It took %1.").arg(Labels::duration(m_seconds)));
    m_seconds = -1;
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
    return isBuilt(targetProject(), targetBoard());
}

bool BuildPanel::isBuilt(const QString &project, const QString &board) const
{
    if (project.isEmpty())
        return false;
    if (project == m_builtProject && board == m_builtBoard)
        return true;
    // Built before this session, and still what the library asks for (build.status).
    return project == m_currentProject && board == m_currentBoard;
}

void BuildPanel::setBuildCurrent(const QString &project, const QString &board, bool current)
{
    if (current) {
        m_currentProject = project;
        m_currentBoard = board;
    } else if (project == m_currentProject && board == m_currentBoard) {
        m_currentProject.clear();
        m_currentBoard.clear();
    }
    updateFlashButton();
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

QString BuildPanel::problemText() const
{
    return m_problems->text();
}

QStringList BuildPanel::visibleLogLines() const
{
    QStringList out;
    for (QTextBlock b = m_log->document()->begin(); b.isValid(); b = b.next()) {
        if (b.isVisible())
            out << b.text();
    }
    return out;
}

QString BuildPanel::flashProgressText(qint64 written, qint64 total)
{
    // The speed since writing started, and the time left at it.
    if (written <= 0 || !m_flashClock.isValid()) {
        m_flashClock.start();
        m_flashFrom = written;
    }
    const int percent = int(written * 100 / std::max<qint64>(total, 1));
    const double seconds = double(m_flashClock.elapsed()) / 1000.0;
    if (seconds < 0.5 || written <= m_flashFrom)
        return tr("Writing %1%").arg(percent);
    const double rate = double(written - m_flashFrom) / seconds;
    const qint64 left = qint64(double(total - written) / rate);
    return tr("Writing %1% · %2/s · about %3 s left")
        .arg(percent)
        .arg(QLocale().formattedDataSize(qint64(rate)))
        .arg(std::max<qint64>(left, 0));
}
