#include "DebugPanel.h"

#include "CoreConnection.h"

#include <QAction>
#include <QFileInfo>
#include <QKeySequence>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QSplitter>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
constexpr int NumberRole = Qt::UserRole + 1;
constexpr int LocationRole = Qt::UserRole + 2;
}

DebugPanel::DebugPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(this))
    , m_continue(new QPushButton(QIcon::fromTheme(u"debug-run"_s, QIcon::fromTheme(u"media-playback-start"_s)),
                                 tr("Continue"), this))
    , m_pause(new QPushButton(QIcon::fromTheme(u"media-playback-pause"_s), tr("Pause"), this))
    , m_next(new QPushButton(QIcon::fromTheme(u"debug-step-over"_s), tr("Step over"), this))
    , m_step(new QPushButton(QIcon::fromTheme(u"debug-step-into"_s), tr("Step into"), this))
    , m_finish(new QPushButton(QIcon::fromTheme(u"debug-step-out"_s), tr("Step out"), this))
    , m_location(new QLineEdit(this))
    , m_add(new QPushButton(QIcon::fromTheme(u"list-add"_s), tr("Add breakpoint"), this))
    , m_remove(new QPushButton(QIcon::fromTheme(u"list-remove"_s), tr("Remove"), this))
    , m_breakpoints(new QListWidget(this))
    , m_stack(new QTreeWidget(this))
    , m_registers(new QTreeWidget(this))
    , m_locals(new QTreeWidget(this))
    , m_watchInput(new QLineEdit(this))
    , m_watches(new QTreeWidget(this))
{
    m_watchInput->setAccessibleName(tr("Expression to watch"));
    m_watchInput->setPlaceholderText(tr("Watch an expression, e.g. count * 2 or buf[3]"));
    m_watchInput->setToolTip(tr("A C or C++ expression, read again every time the firmware stops"));
    m_watches->setAccessibleName(tr("Watched expressions"));
    m_watches->setHeaderLabels({tr("Expression"), tr("Value")});
    m_watches->setRootIsDecorated(false);
    m_watches->setContextMenuPolicy(Qt::ActionsContextMenu);
    auto *forget = new QAction(QIcon::fromTheme(u"list-remove"_s), tr("Stop watching"), m_watches);
    forget->setShortcut(QKeySequence::Delete);
    forget->setShortcutContext(Qt::WidgetShortcut);
    m_watches->addAction(forget);
    connect(forget, &QAction::triggered, this, [this] { delete m_watches->currentItem(); });
    connect(m_watchInput, &QLineEdit::returnPressed, this, [this] {
        addWatch(m_watchInput->text().trimmed());
        m_watchInput->clear();
    });
    connect(m_core, &CoreConnection::debugEvaluated, this,
            [this](const QString &expression, const QString &value, const QString &error) {
                for (int i = 0; i < m_watches->topLevelItemCount(); ++i) {
                    QTreeWidgetItem *item = m_watches->topLevelItem(i);
                    if (item->text(0) != expression)
                        continue;
                    item->setText(1, error.isEmpty() ? value : error);
                    item->setForeground(1, palette().brush(error.isEmpty() ? QPalette::Text : QPalette::PlaceholderText));
                    item->setToolTip(1, error.isEmpty() ? value : error);
                }
            });
    m_locals->setAccessibleName(tr("Local variables"));
    m_locals->setHeaderLabels({tr("Variable"), tr("Value"), tr("Type")});
    m_locals->setRootIsDecorated(false);
    m_locals->setToolTip(tr("The arguments and local variables of the function the firmware stopped in"));
    m_location->setAccessibleName(tr("Breakpoint location"));
    m_breakpoints->setAccessibleName(tr("Breakpoints"));
    m_stack->setAccessibleName(tr("Call stack"));
    m_registers->setAccessibleName(tr("Registers"));
    m_status->setWordWrap(true);
    m_status->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_continue->setToolTip(tr("Run until a breakpoint or Pause"));
    m_next->setToolTip(tr("Run to the next line, stepping over calls"));
    m_step->setToolTip(tr("Run to the next line, stepping into calls"));
    m_finish->setToolTip(tr("Run until the current function returns"));
    m_location->setPlaceholderText(tr("Function or file:line, e.g. setup or blink.ino:12"));
    m_add->setToolTip(tr("Breakpoints can be added while the firmware is paused"));

    const QFont fixed = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    m_stack->setHeaderLabels({tr("Function"), tr("Location")});
    m_stack->setRootIsDecorated(false);
    m_registers->setHeaderLabels({tr("Register"), tr("Value")});
    m_registers->setRootIsDecorated(false);
    m_registers->setFont(fixed);
    m_registers->header()->setSectionResizeMode(0, QHeaderView::ResizeToContents);

    connect(m_continue, &QPushButton::clicked, this, [this] { m_core->debugControl(u"continue"_s); });
    connect(m_pause, &QPushButton::clicked, this, [this] { m_core->debugControl(u"interrupt"_s); });
    connect(m_next, &QPushButton::clicked, this, [this] { m_core->debugControl(u"next"_s); });
    connect(m_step, &QPushButton::clicked, this, [this] { m_core->debugControl(u"step"_s); });
    connect(m_finish, &QPushButton::clicked, this, [this] { m_core->debugControl(u"finish"_s); });
    connect(m_add, &QPushButton::clicked, this, &DebugPanel::addBreakpoint);
    connect(m_location, &QLineEdit::returnPressed, this, &DebugPanel::addBreakpoint);
    connect(m_remove, &QPushButton::clicked, this, [this] {
        if (QListWidgetItem *item = m_breakpoints->currentItem())
            m_core->debugDelete(item->data(NumberRole).toInt());
    });
    connect(m_breakpoints, &QListWidget::currentItemChanged, this, [this] { setMode(m_mode); });

    auto *controls = new QHBoxLayout;
    for (QPushButton *b : {m_continue, m_pause, m_next, m_step, m_finish})
        controls->addWidget(b);
    controls->addStretch(1);
    // The location gets a row of its own, so its hint is not cut short.
    auto *add = new QHBoxLayout;
    add->addStretch(1);
    add->addWidget(m_add);
    add->addWidget(m_remove);

    auto *bpBox = new QWidget(this);
    auto *bpLayout = new QVBoxLayout(bpBox);
    bpLayout->setContentsMargins(0, 0, 0, 0);
    bpLayout->addWidget(new QLabel(tr("Breakpoints"), bpBox));
    bpLayout->addWidget(m_location);
    bpLayout->addLayout(add);
    bpLayout->addWidget(m_breakpoints, 1);
    auto *stackBox = new QWidget(this);
    auto *stackLayout = new QVBoxLayout(stackBox);
    stackLayout->setContentsMargins(0, 0, 0, 0);
    stackLayout->addWidget(new QLabel(tr("Call stack"), stackBox));
    stackLayout->addWidget(m_stack, 1);

    auto *localsBox = new QWidget(this);
    auto *localsLayout = new QVBoxLayout(localsBox);
    localsLayout->setContentsMargins(0, 0, 0, 0);
    localsLayout->addWidget(new QLabel(tr("Local variables"), localsBox));
    localsLayout->addWidget(m_locals, 2);
    localsLayout->addWidget(new QLabel(tr("Watch"), localsBox));
    localsLayout->addWidget(m_watchInput);
    localsLayout->addWidget(m_watches, 1);
    m_watches->setFont(fixed);
    m_locals->setFont(fixed);
    m_locals->header()->setSectionResizeMode(QHeaderView::ResizeToContents);

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(bpBox);
    splitter->addWidget(stackBox);
    splitter->addWidget(localsBox);
    splitter->addWidget(m_registers);
    for (int i = 0; i < 4; ++i)
        splitter->setStretchFactor(i, 1);
    // Wide enough for a register's name and a 32-bit value, and a function and its file.
    m_registers->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_registers->setMinimumWidth(m_registers->fontMetrics().averageCharWidth() * 26);
    m_stack->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_stack->setMinimumWidth(m_stack->fontMetrics().averageCharWidth() * 24);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_status);
    layout->addLayout(controls);
    layout->addWidget(splitter, 1);

    connect(m_core, &CoreConnection::debugStarted, this, [this] {
        m_breakpoints->clear();
        m_stack->clear();
        m_registers->clear();
        m_locals->clear();
        setMode(Mode::Paused);
        // The kept breakpoints, set while the firmware has not started yet.
        m_restoring = int(m_kept.size());
        for (const QString &loc : std::as_const(m_kept))
            m_core->debugBreak(loc);
    });
    connect(m_core, &CoreConnection::breakpointsListed, this, [this](const QString &project, const QStringList &list) {
        if (project != m_project || m_mode != Mode::Off)
            return;
        m_kept = list;
        emit breakpointsChanged();
    });
    connect(m_core, &CoreConnection::debugStopped, this, &DebugPanel::stopped);
    connect(m_core, &CoreConnection::debugRunning, this, [this] {
        m_status->setText(tr("Running."));
        setMode(Mode::Running);
    });
    connect(m_core, &CoreConnection::debugExited, this, [this] {
        m_status->setText(tr("GDB has stopped; the simulator keeps running without the debugger."));
        setMode(Mode::Off);
        m_core->getBreakpoints(m_project); // the project shown may have changed meanwhile
    });
    connect(m_core, &CoreConnection::simStopped, this, [this] {
        if (m_mode == Mode::Off)
            return;
        m_status->setText(tr("The simulator has stopped."));
        setMode(Mode::Off);
        m_core->getBreakpoints(m_project);
    });
    connect(m_core, &CoreConnection::breakpointSet, this, [this](const QJsonObject &bp, const QString &location) {
        const int number = bp.value(u"number"_s).toInt();
        auto *item = new QListWidgetItem(tr("%1: %2").arg(number).arg(where(bp)), m_breakpoints);
        item->setData(NumberRole, number);
        item->setData(LocationRole, location);
        item->setToolTip(bp.value(u"file"_s).toString());
        if (m_restoring > 0) {
            --m_restoring; // one of the kept ones
            return;
        }
        m_location->clear();
        if (!location.isEmpty() && !m_kept.contains(location))
            keep(m_kept + QStringList{location});
    });
    connect(m_core, &CoreConnection::breakpointDeleted, this, [this](int number) {
        for (int i = 0; i < m_breakpoints->count(); ++i) {
            if (m_breakpoints->item(i)->data(NumberRole).toInt() != number)
                continue;
            const QString location = m_breakpoints->item(i)->data(LocationRole).toString();
            delete m_breakpoints->takeItem(i);
            QStringList left = m_kept;
            left.removeAll(location);
            keep(left);
            break;
        }
        setMode(m_mode);
    });
    connect(m_core, &CoreConnection::debugInspected, this, &DebugPanel::showInspection);
    connect(m_core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method == u"debug.break" && m_restoring > 0)
            --m_restoring; // a kept one that no longer fits the build; it stays kept
        if (method.startsWith(u"debug."))
            m_status->setText(tr("Debugger: %1").arg(message));
    });
    m_status->setText(tr("Not debugging. Build a project for an ESP32, ESP32-S3 or ESP32-C3 board, "
                         "then choose Debug in the toolbar."));
    setMode(Mode::Off);
}

QString DebugPanel::statusText() const
{
    return m_status->text();
}

void DebugPanel::setProject(const QString &project)
{
    if (project == m_project)
        return;
    m_project = project;
    if (m_mode == Mode::Off) {
        m_kept.clear();
        emit breakpointsChanged();
        m_core->getBreakpoints(project);
    }
}

void DebugPanel::keep(const QStringList &locations)
{
    m_kept = locations;
    m_core->setBreakpoints(m_project, m_kept);
    emit breakpointsChanged();
}

bool DebugPanel::runTo(const QString &location)
{
    if (m_mode != Mode::Paused) {
        m_status->setText(m_mode == Mode::Running ? tr("Pause the firmware first, then run to a line.")
                                                  : tr("Run to a line works while debugging: choose Debug in the toolbar."));
        return false;
    }
    m_status->setText(tr("Running to %1…").arg(location));
    m_core->debugRunTo(location);
    return true;
}

void DebugPanel::addWatch(const QString &expression)
{
    if (expression.isEmpty())
        return;
    for (int i = 0; i < m_watches->topLevelItemCount(); ++i) {
        if (m_watches->topLevelItem(i)->text(0) == expression)
            return;
    }
    auto *item = new QTreeWidgetItem(m_watches);
    item->setText(0, expression);
    if (m_mode == Mode::Paused)
        m_core->debugEvaluate(expression);
    else
        item->setText(1, tr("(read when the firmware stops)"));
}

void DebugPanel::evaluateWatches()
{
    for (int i = 0; i < m_watches->topLevelItemCount(); ++i)
        m_core->debugEvaluate(m_watches->topLevelItem(i)->text(0));
}

bool DebugPanel::toggleBreakpoint(const QString &location)
{
    if (m_mode == Mode::Running) {
        m_status->setText(tr("Pause the firmware first: GDB changes breakpoints only while it is stopped."));
        return false;
    }
    if (m_mode == Mode::Paused) {
        for (int i = 0; i < m_breakpoints->count(); ++i) {
            if (m_breakpoints->item(i)->data(LocationRole).toString() == location) {
                m_core->debugDelete(m_breakpoints->item(i)->data(NumberRole).toInt());
                return true;
            }
        }
        m_core->debugBreak(location);
        return true;
    }
    QStringList list = m_kept;
    if (!list.removeAll(location))
        list << location;
    keep(list);
    return true;
}

void DebugPanel::addBreakpoint()
{
    const QString loc = m_location->text().trimmed();
    if (loc.isEmpty() || m_mode != Mode::Paused)
        return;
    m_core->debugBreak(loc);
}

QString DebugPanel::where(const QJsonObject &frame)
{
    const QString func = frame.value(u"func"_s).toString();
    const QString file = QFileInfo(frame.value(u"file"_s).toString()).fileName();
    const int line = frame.value(u"line"_s).toInt();
    if (!func.isEmpty() && !file.isEmpty() && line > 0)
        return tr("%1 (%2, line %3)").arg(func, file).arg(line);
    if (!func.isEmpty())
        return func;
    return frame.value(u"addr"_s).toString();
}

void DebugPanel::stopped(const QString &reason, const QJsonObject &frame, int breakpoint)
{
    // A breakpoint of the list (not Run to a line's temporary one).
    bool listed = false;
    for (int i = 0; i < m_breakpoints->count(); ++i)
        listed = listed || m_breakpoints->item(i)->data(NumberRole).toInt() == breakpoint;
    if (reason == u"paused" && frame.value(u"func"_s).toString().isEmpty())
        m_status->setText(tr("Paused before the firmware starts. Add breakpoints, then Continue."));
    else if (reason == u"breakpoint-hit" && listed)
        m_status->setText(tr("Stopped at breakpoint %1 in %2.").arg(breakpoint).arg(where(frame)));
    else
        m_status->setText(tr("Paused in %1.").arg(where(frame)));
    for (int i = 0; i < m_breakpoints->count(); ++i) {
        if (breakpoint > 0 && m_breakpoints->item(i)->data(NumberRole).toInt() == breakpoint)
            m_breakpoints->setCurrentRow(i);
    }
    setMode(Mode::Paused);
    m_core->debugInspect();
    evaluateWatches();
}

void DebugPanel::showInspection(const QJsonArray &frames, const QJsonArray &registers, const QJsonArray &locals)
{
    m_locals->clear();
    for (const QJsonValue &v : locals) {
        const QJsonObject l = v.toObject();
        auto *item = new QTreeWidgetItem(m_locals);
        item->setText(0, l.value(u"arg"_s).toBool() ? tr("%1 (argument)").arg(l.value(u"name"_s).toString())
                                                    : l.value(u"name"_s).toString());
        if (l.contains(u"value"_s)) {
            item->setText(1, l.value(u"value"_s).toString());
        } else {
            item->setText(1, tr("…"));
            item->setToolTip(1, tr("GDB shows this kind of value (a structure or an array) only in parts"));
        }
        item->setText(2, l.value(u"type"_s).toString());
    }
    m_stack->clear();
    for (const QJsonValue &v : frames) {
        const QJsonObject f = v.toObject();
        const QString file = QFileInfo(f.value(u"file"_s).toString()).fileName();
        const int line = f.value(u"line"_s).toInt();
        auto *item = new QTreeWidgetItem(m_stack);
        item->setText(0, f.value(u"func"_s).toString(f.value(u"addr"_s).toString()));
        item->setText(1, line > 0 ? tr("%1:%2").arg(file).arg(line) : file);
        item->setToolTip(1, f.value(u"file"_s).toString());
    }
    m_registers->clear();
    for (const QJsonValue &v : registers) {
        const QJsonObject r = v.toObject();
        auto *item = new QTreeWidgetItem(m_registers);
        item->setText(0, r.value(u"name"_s).toString());
        item->setText(1, r.value(u"value"_s).toString());
    }
}

void DebugPanel::setMode(Mode mode)
{
    m_mode = mode;
    const bool paused = mode == Mode::Paused;
    m_continue->setEnabled(paused);
    m_next->setEnabled(paused);
    m_step->setEnabled(paused);
    m_finish->setEnabled(paused);
    m_pause->setEnabled(mode == Mode::Running);
    // GDB takes breakpoints only while the target is stopped.
    m_location->setEnabled(paused);
    m_add->setEnabled(paused);
    m_remove->setEnabled(paused && m_breakpoints->currentItem());
    if (mode == Mode::Running) {
        m_stack->clear();
        m_registers->clear();
        m_locals->clear();
    }
    if (mode != Mode::Paused) {
        for (int i = 0; i < m_watches->topLevelItemCount(); ++i) {
            m_watches->topLevelItem(i)->setText(1, QString());
            m_watches->topLevelItem(i)->setToolTip(1, QString());
        }
    }
}
