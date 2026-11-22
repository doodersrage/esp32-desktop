#include "DebugPanel.h"

#include "CoreConnection.h"

#include <QCheckBox>
#include <QRegularExpression>
#include <QSettings>
#include <QAction>

#include <functional>
#include <QFileInfo>
#include <QInputDialog>
#include <QKeySequence>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPlainTextEdit>
#include <QSignalBlocker>
#include <QPushButton>
#include <QSplitter>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
constexpr int NumberRole = Qt::UserRole + 1;
constexpr int LocationRole = Qt::UserRole + 2;
// On a variable's row: an expression for it, to list its members or set it.
constexpr int ExprRole = Qt::UserRole + 3;
// Every row of a tree, children too.
void eachItem(QTreeWidget *tree, const std::function<void(QTreeWidgetItem *)> &f)
{
    std::function<void(QTreeWidgetItem *)> walk = [&](QTreeWidgetItem *i) {
        f(i);
        for (int c = 0; c < i->childCount(); ++c)
            walk(i->child(c));
    };
    for (int i = 0; i < tree->topLevelItemCount(); ++i)
        walk(tree->topLevelItem(i));
}
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
    , m_hex(new QCheckBox(tr("Hex"), this))
    , m_memoryInput(new QLineEdit(this))
    , m_memory(new QPlainTextEdit(this))
{
    m_hex->setToolTip(tr("Show whole numbers in the variables and watches as hex"));
    m_hex->setChecked(QSettings().value(u"debugger/hex"_s, false).toBool());
    connect(m_hex, &QCheckBox::toggled, this, [this](bool on) {
        QSettings().setValue(u"debugger/hex"_s, on);
        reformatValues();
    });
    m_memoryInput->setAccessibleName(tr("Memory address"));
    m_memoryInput->setPlaceholderText(tr("Address or expression, e.g. &config or buf"));
    m_memoryInput->setToolTip(tr("Show 256 bytes of memory from there, read again every time the firmware stops"));
    m_memory->setAccessibleName(tr("Memory"));
    m_memory->setReadOnly(true);
    m_memory->setLineWrapMode(QPlainTextEdit::NoWrap);
    m_memory->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    connect(m_memoryInput, &QLineEdit::returnPressed, this, &DebugPanel::readMemory);
    connect(m_core, &CoreConnection::memoryRead, this,
            [this](const QString &asked, const QString &address, const QByteArray &bytes) {
                if (asked == m_memoryInput->text().trimmed())
                    m_memory->setPlainText(hexDump(address.toULongLong(nullptr, 16), bytes));
            });
    auto *stopOnChange = new QAction(tr("Stop when it changes"), m_watches);
    stopOnChange->setToolTip(tr("A watchpoint: the firmware stops the moment this value changes"));
    m_watches->addAction(stopOnChange);
    connect(stopOnChange, &QAction::triggered, this, [this] {
        if (QTreeWidgetItem *item = m_watches->currentItem())
            watchChanges(item->text(0));
    });
    connect(m_core, &CoreConnection::watchpointSet, this, [this](int number, const QString &expression) {
        auto *item = new QListWidgetItem(tr("%1: when %2 changes").arg(number).arg(expression), m_breakpoints);
        item->setData(NumberRole, number);
        item->setIcon(QIcon::fromTheme(u"view-refresh"_s));
    });
    m_watchInput->setAccessibleName(tr("Expression to watch"));
    m_watchInput->setPlaceholderText(tr("Watch an expression, e.g. count * 2 or buf[3]"));
    m_watchInput->setToolTip(tr("A C or C++ expression, read again every time the firmware stops"));
    m_watches->setAccessibleName(tr("Watched expressions"));
    m_watches->setHeaderLabels({tr("Expression"), tr("Value")});

    m_watches->setContextMenuPolicy(Qt::ActionsContextMenu);
    auto *forget = new QAction(QIcon::fromTheme(u"list-remove"_s), tr("Stop watching"), m_watches);
    forget->setShortcut(QKeySequence::Delete);
    forget->setShortcutContext(Qt::WidgetShortcut);
    m_watches->addAction(forget);
    connect(forget, &QAction::triggered, this, [this] {
        if (QTreeWidgetItem *item = m_watches->currentItem())
            removeWatch(item->text(0));
    });
    connect(m_core, &CoreConnection::watchesListed, this, [this](const QString &project, const QStringList &list) {
        if (project != m_project)
            return;
        m_watches->clear();
        for (const QString &e : list) {
            auto *item = new QTreeWidgetItem(m_watches);
            item->setText(0, e);
            item->setData(0, ExprRole, e);
        }
        if (m_mode == Mode::Paused)
            evaluateWatches();
    });
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
                    if (error.isEmpty())
                        setValue(item, 1, value);
                    else
                        item->setText(1, error);
                    // A structure or an array prints as {…}: it can be opened.
                    qDeleteAll(item->takeChildren());
                    item->setChildIndicatorPolicy(error.isEmpty() && value.startsWith(u'{')
                                                      ? QTreeWidgetItem::ShowIndicator
                                                      : QTreeWidgetItem::DontShowIndicator);
                    if (error.isEmpty())
                        markChanged(item, 1, expression, m_lastWatches);
                    item->setForeground(1, palette().brush(error.isEmpty() ? QPalette::Text : QPalette::PlaceholderText));
                    item->setToolTip(1, error.isEmpty() ? value : error);
                }
            });
    m_locals->setAccessibleName(tr("Local variables"));
    m_locals->setHeaderLabels({tr("Variable"), tr("Value"), tr("Type")});
    m_locals->setRootIsDecorated(true);
    m_watches->setRootIsDecorated(true);
    for (QTreeWidget *tree : {m_locals, m_watches}) {
        connect(tree, &QTreeWidget::itemExpanded, this, &DebugPanel::expandItem);
        connect(tree, &QTreeWidget::itemDoubleClicked, this, &DebugPanel::editValue);
    }
    connect(m_core, &CoreConnection::childrenListed, this, &DebugPanel::showChildren);
    connect(m_core, &CoreConnection::assigned, this, [this](const QString &expression, const QString &value) {
        m_status->setText(tr("%1 is now %2.").arg(expression, value));
        m_core->debugInspect(m_frame); // everything that follows from it, again
    });
    m_locals->setToolTip(tr("The arguments and local variables of the function the firmware stopped in. Open a "
                            "structure or an array to see its members; double-click a value to set it."));
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
    m_breakpoints->setContextMenuPolicy(Qt::ActionsContextMenu);
    auto *condition = new QAction(tr("Condition…"), m_breakpoints);
    condition->setToolTip(tr("Stop here only when an expression is true"));
    m_breakpoints->addAction(condition);
    connect(condition, &QAction::triggered, this, [this] {
        if (QListWidgetItem *item = m_breakpoints->currentItem())
            editCondition(keptLocation(item->data(LocationRole).toString()));
    });
    auto *ignore = new QAction(tr("Pass the next times…"), m_breakpoints);
    ignore->setToolTip(tr("Run past this breakpoint a number of times before it stops again"));
    m_breakpoints->addAction(ignore);
    connect(ignore, &QAction::triggered, this, [this] {
        QListWidgetItem *item = m_breakpoints->currentItem();
        if (!item)
            return;
        bool ok = false;
        const int count = QInputDialog::getInt(this, tr("Pass the breakpoint"),
                                               tr("Run past it this many times, then stop:"), 10, 0, 1000000, 1, &ok);
        if (ok)
            ignoreBreakpoint(item->data(NumberRole).toInt(), count);
    });

    auto *controls = new QHBoxLayout;
    for (QPushButton *b : {m_continue, m_pause, m_next, m_step, m_finish})
        controls->addWidget(b);
    controls->addStretch(1);
    controls->addWidget(m_hex);
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
    m_stack->setToolTip(tr("Click a function to see its variables and the line it is at"));
    connect(m_stack, &QTreeWidget::itemClicked, this,
            [this](QTreeWidgetItem *item) { selectFrame(m_stack->indexOfTopLevelItem(item)); });
    connect(m_stack, &QTreeWidget::itemActivated, this,
            [this](QTreeWidgetItem *item) { selectFrame(m_stack->indexOfTopLevelItem(item)); });

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
    auto *memoryBox = new QWidget(this);
    auto *memoryLayout = new QVBoxLayout(memoryBox);
    memoryLayout->setContentsMargins(0, 0, 0, 0);
    memoryLayout->addWidget(new QLabel(tr("Memory"), memoryBox));
    memoryLayout->addWidget(m_memoryInput);
    memoryLayout->addWidget(m_memory, 1);
    splitter->addWidget(memoryBox);
    for (int i = 0; i < 5; ++i)
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
        for (const QString &kept : std::as_const(m_kept))
            m_core->debugBreak(keptLocation(kept), keptCondition(kept));
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
        const QString condition = bp.value(u"condition"_s).toString();
        const QString kept = condition.isEmpty() ? location : u"%1 if %2"_s.arg(location, condition);
        auto *item = new QListWidgetItem(condition.isEmpty() ? tr("%1: %2").arg(number).arg(where(bp))
                                                             : tr("%1: %2, if %3").arg(number).arg(where(bp), condition),
                                         m_breakpoints);
        item->setData(NumberRole, number);
        item->setData(LocationRole, kept);
        item->setToolTip(bp.value(u"file"_s).toString());
        if (m_restoring > 0) {
            --m_restoring; // one of the kept ones
            return;
        }
        m_location->clear();
        if (!location.isEmpty() && !m_kept.contains(kept)) {
            // In place of the same location's breakpoint with another condition.
            QStringList list;
            for (const QString &k : std::as_const(m_kept))
                if (keptLocation(k) != location)
                    list << k;
            keep(list + QStringList{kept});
        }
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
    m_watches->clear();
    m_core->getWatches(project);
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
    if (watchExpressions().contains(expression))
        return;
    auto *item = new QTreeWidgetItem(m_watches);
    item->setText(0, expression);
    item->setData(0, ExprRole, expression);
    if (m_mode == Mode::Paused)
        m_core->debugEvaluate(expression);
    else
        item->setText(1, tr("(read when the firmware stops)"));
    keepWatches();
}

void DebugPanel::removeWatch(const QString &expression)
{
    for (int i = 0; i < m_watches->topLevelItemCount(); ++i) {
        if (m_watches->topLevelItem(i)->text(0) == expression) {
            delete m_watches->takeTopLevelItem(i);
            keepWatches();
            return;
        }
    }
}

QStringList DebugPanel::watchExpressions() const
{
    QStringList out;
    for (int i = 0; i < m_watches->topLevelItemCount(); ++i)
        out << m_watches->topLevelItem(i)->text(0);
    return out;
}

void DebugPanel::keepWatches()
{
    m_core->setWatches(m_project, watchExpressions());
}

QString DebugPanel::keptLocation(const QString &kept)
{
    return kept.section(u" if "_s, 0, 0).trimmed();
}

QString DebugPanel::keptCondition(const QString &kept)
{
    return kept.contains(u" if "_s) ? kept.section(u" if "_s, 1).trimmed() : QString();
}

void DebugPanel::editCondition(const QString &location)
{
    QString current;
    for (const QString &k : std::as_const(m_kept)) {
        if (keptLocation(k) == location)
            current = keptCondition(k);
    }
    bool ok = false;
    const QString condition = QInputDialog::getText(
        this, tr("Breakpoint condition"),
        tr("Stop at %1 only when this is true (a C expression, e.g. i == 10); empty to always stop:").arg(location),
        QLineEdit::Normal, current, &ok);
    if (ok)
        setCondition(location, condition.trimmed());
}

bool DebugPanel::setCondition(const QString &location, const QString &condition)
{
    const QString kept = condition.isEmpty() ? location : u"%1 if %2"_s.arg(location, condition);
    if (m_mode == Mode::Running) {
        m_status->setText(tr("Pause the firmware first: GDB changes breakpoints only while it is stopped."));
        return false;
    }
    if (m_mode == Mode::Paused) {
        // GDB's breakpoint again, with the new condition.
        for (int i = 0; i < m_breakpoints->count(); ++i) {
            if (keptLocation(m_breakpoints->item(i)->data(LocationRole).toString()) == location)
                m_core->debugDelete(m_breakpoints->item(i)->data(NumberRole).toInt());
        }
        m_core->debugBreak(location, condition);
        return true;
    }
    QStringList list;
    bool replaced = false;
    for (const QString &k : std::as_const(m_kept)) {
        if (keptLocation(k) == location) {
            if (!replaced)
                list << kept;
            replaced = true;
        } else {
            list << k;
        }
    }
    if (!replaced)
        list << kept;
    keep(list);
    return true;
}

void DebugPanel::markChanged(QTreeWidgetItem *item, int column, const QString &key, QHash<QString, QString> &last)
{
    // Only where the firmware stopped: another frame's values are not "since the last stop".
    if (m_frame != 0)
        return;
    const QVariant raw = item->data(column, Qt::UserRole);
    const QString value = raw.isValid() ? raw.toString() : item->text(column);
    const auto was = last.constFind(key);
    const bool changed = was != last.constEnd() && *was != value;
    QFont font = item->font(column);
    font.setBold(changed);
    item->setFont(column, font);
    item->setToolTip(column, changed ? tr("Changed since the last stop; it was %1").arg(*was) : value);
    last.insert(key, value);
}

void DebugPanel::expandItem(QTreeWidgetItem *item)
{
    const QString expression = item->data(0, ExprRole).toString();
    if (item->childCount() > 0 || expression.isEmpty() || m_mode != Mode::Paused)
        return;
    item->setExpanded(true);
    m_core->debugChildren(expression);
}

void DebugPanel::showChildren(const QString &expression, const QJsonArray &children)
{
    for (QTreeWidget *tree : {m_locals, m_watches}) {
        eachItem(tree, [&](QTreeWidgetItem *item) {
            if (item->data(0, ExprRole).toString() != expression || item->childCount() > 0)
                return;
            for (const QJsonValue &v : children) {
                const QJsonObject c = v.toObject();
                auto *child = new QTreeWidgetItem(item);
                child->setText(0, c.value(u"name"_s).toString());
                child->setData(0, ExprRole, c.value(u"expression"_s).toString());
                if (c.contains(u"value"_s))
                    setValue(child, 1, c.value(u"value"_s).toString());
                else
                    child->setText(1, tr("…"));
                if (tree->columnCount() > 2)
                    child->setText(2, c.value(u"type"_s).toString());
                child->setToolTip(0, c.value(u"type"_s).toString());
                child->setChildIndicatorPolicy(c.value(u"children"_s).toInt() > 0 ? QTreeWidgetItem::ShowIndicator
                                                                                  : QTreeWidgetItem::DontShowIndicator);
            }
            if (children.isEmpty())
                item->setChildIndicatorPolicy(QTreeWidgetItem::DontShowIndicator);
        });
    }
}

void DebugPanel::editValue(QTreeWidgetItem *item, int column)
{
    const QString expression = item->data(0, ExprRole).toString();
    const QVariant raw = item->data(1, Qt::UserRole);
    if (column != 1 || expression.isEmpty() || !raw.isValid() || m_mode != Mode::Paused)
        return;
    bool ok = false;
    const QString value = QInputDialog::getText(this, tr("Set a value"),
                                                tr("New value for %1 (a C expression):").arg(expression),
                                                QLineEdit::Normal, raw.toString(), &ok);
    if (ok && !value.trimmed().isEmpty())
        assign(expression, value.trimmed());
}

bool DebugPanel::assign(const QString &expression, const QString &value)
{
    if (m_mode != Mode::Paused) {
        m_status->setText(tr("Pause the firmware first: values can be set only while it is stopped."));
        return false;
    }
    m_core->debugAssign(expression, value);
    return true;
}

bool DebugPanel::ignoreBreakpoint(int number, int count)
{
    if (m_mode != Mode::Paused) {
        m_status->setText(tr("Pause the firmware first: GDB changes breakpoints only while it is stopped."));
        return false;
    }
    m_core->debugIgnore(number, count);
    m_status->setText(count > 0 ? tr("Breakpoint %1: passes left before it stops: %2.").arg(number).arg(count)
                                : tr("Breakpoint %1 stops again each time.").arg(number));
    return true;
}

QString DebugPanel::shownValue(const QString &value, bool hex)
{
    if (!hex)
        return value;
    // A whole number as GDB prints it ("42", "-3"), as hex; anything else as it is.
    static const QRegularExpression whole(u"^-?\\d+$"_s);
    if (!whole.match(value).hasMatch())
        return value;
    bool ok = false;
    const qlonglong n = value.toLongLong(&ok);
    if (!ok)
        return value;
    return n < 0 ? u"-0x%1"_s.arg(QString::number(-n, 16)) : u"0x%1"_s.arg(QString::number(n, 16));
}

void DebugPanel::setValue(QTreeWidgetItem *item, int column, const QString &value)
{
    item->setData(column, Qt::UserRole, value);
    item->setText(column, shownValue(value, m_hex->isChecked()));
}

void DebugPanel::reformatValues()
{
    for (QTreeWidget *tree : {m_locals, m_watches}) {
        eachItem(tree, [this](QTreeWidgetItem *item) {
            const QVariant raw = item->data(1, Qt::UserRole);
            if (raw.isValid())
                item->setText(1, shownValue(raw.toString(), m_hex->isChecked()));
        });
    }
}

bool DebugPanel::watchChanges(const QString &expression)
{
    if (m_mode != Mode::Paused) {
        m_status->setText(tr("Pause the firmware first: GDB sets watchpoints only while it is stopped."));
        return false;
    }
    m_core->debugWatch(expression);
    return true;
}

void DebugPanel::selectFrame(int level)
{
    if (m_mode != Mode::Paused || level < 0 || level >= m_frames.size())
        return;
    m_core->debugInspect(level);
}

void DebugPanel::readMemory()
{
    const QString at = m_memoryInput->text().trimmed();
    if (at.isEmpty() || m_mode != Mode::Paused)
        return;
    m_core->debugMemory(at, 256);
}

QString DebugPanel::memoryText() const
{
    return m_memory->toPlainText();
}

QString DebugPanel::hexDump(quint64 address, const QByteArray &bytes)
{
    QStringList rows;
    for (qsizetype i = 0; i < bytes.size(); i += 16) {
        const QByteArray row = bytes.mid(i, 16);
        QString hex, text;
        for (qsizetype j = 0; j < 16; ++j) {
            hex += j < row.size() ? u"%1 "_s.arg(quint8(row.at(j)), 2, 16, QLatin1Char('0')) : u"   "_s;
            if (j == 7)
                hex += u' ';
            if (j < row.size())
                text += row.at(j) >= 0x20 && row.at(j) < 0x7f ? QChar::fromLatin1(row.at(j)) : QChar(u'.');
        }
        rows << u"%1  %2 %3"_s.arg(address + quint64(i), 8, 16, QLatin1Char('0')).arg(hex, text);
    }
    return rows.join(u'\n');
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
            if (keptLocation(m_breakpoints->item(i)->data(LocationRole).toString()) == location) {
                m_core->debugDelete(m_breakpoints->item(i)->data(NumberRole).toInt());
                return true;
            }
        }
        m_core->debugBreak(location);
        return true;
    }
    QStringList list;
    for (const QString &k : std::as_const(m_kept))
        if (keptLocation(k) != location)
            list << k;
    if (list.size() == m_kept.size())
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

void DebugPanel::stopped(const QString &reason, const QJsonObject &frame, int breakpoint, const QJsonObject &watch,
                         int hits)
{
    // A breakpoint of the list (not Run to a line's temporary one).
    bool listed = false;
    for (int i = 0; i < m_breakpoints->count(); ++i)
        listed = listed || m_breakpoints->item(i)->data(NumberRole).toInt() == breakpoint;
    if (reason == u"paused" && frame.value(u"func"_s).toString().isEmpty())
        m_status->setText(tr("Paused before the firmware starts. Add breakpoints, then Continue."));
    else if (!watch.isEmpty())
        m_status->setText(tr("%1 changed from %2 to %3, in %4.")
                              .arg(watch.value(u"expression"_s).toString(), watch.value(u"old"_s).toString(tr("?")),
                                   watch.value(u"new"_s).toString(tr("?")), where(frame)));
    else if (reason == u"breakpoint-hit" && listed && hits > 1)
        m_status->setText(tr("Stopped at breakpoint %1 in %2; it has been reached %3 times.")
                              .arg(breakpoint)
                              .arg(where(frame))
                              .arg(hits));
    else if (reason == u"breakpoint-hit" && listed)
        m_status->setText(tr("Stopped at breakpoint %1 in %2.").arg(breakpoint).arg(where(frame)));
    else
        m_status->setText(tr("Paused in %1.").arg(where(frame)));
    for (int i = 0; i < m_breakpoints->count(); ++i) {
        QListWidgetItem *item = m_breakpoints->item(i);
        if (breakpoint > 0 && item->data(NumberRole).toInt() == breakpoint) {
            m_breakpoints->setCurrentRow(i);
            if (hits > 1)
                item->setToolTip(tr("Reached %1 times in this run").arg(hits));
        }
    }
    setMode(Mode::Paused);
    m_frame = 0;
    m_core->debugInspect(0);
    readMemory();
}

void DebugPanel::showInspection(const QJsonArray &frames, const QJsonArray &registers, const QJsonArray &locals,
                                int frame)
{
    m_frame = frame;
    m_frames = frames;
    m_locals->clear();
    for (const QJsonValue &v : locals) {
        const QJsonObject l = v.toObject();
        auto *item = new QTreeWidgetItem(m_locals);
        item->setText(0, l.value(u"arg"_s).toBool() ? tr("%1 (argument)").arg(l.value(u"name"_s).toString())
                                                    : l.value(u"name"_s).toString());
        item->setData(0, ExprRole, l.value(u"name"_s).toString());
        if (l.contains(u"value"_s)) {
            setValue(item, 1, l.value(u"value"_s).toString());
        } else {
            item->setText(1, tr("…"));
            item->setToolTip(1, tr("A structure or an array: open it to see its members"));
            item->setChildIndicatorPolicy(QTreeWidgetItem::ShowIndicator);
        }
        item->setText(2, l.value(u"type"_s).toString());
        markChanged(item, 1, l.value(u"name"_s).toString(), m_lastLocals);
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
        markChanged(item, 1, r.value(u"name"_s).toString(), m_lastRegisters);
    }
    // The frame looked in, selected; its watches read there.
    if (QTreeWidgetItem *at = m_stack->topLevelItem(frame)) {
        const QSignalBlocker block(m_stack);
        m_stack->setCurrentItem(at);
    }
    emit frameSelected(frames.at(frame).toObject());
    evaluateWatches();
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
