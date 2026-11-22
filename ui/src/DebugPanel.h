// The debugger: run controls, breakpoints, call stack and registers for a build
// running in the simulator under GDB. The core runs GDB; this renders what it reports.

#pragma once

#include <QHash>
#include <QJsonArray>
#include <QJsonObject>
#include <QWidget>

class CoreConnection;
class QCheckBox;
class QLabel;
class QLineEdit;
class QListWidget;
class QPlainTextEdit;
class QTreeWidgetItem;
class QPushButton;
class QTreeWidget;

class DebugPanel : public QWidget {
    Q_OBJECT

public:
    explicit DebugPanel(CoreConnection *core, QWidget *parent = nullptr);

    QString statusText() const;
    QLineEdit *locationInput() const { return m_location; }
    QPushButton *addButton() const { return m_add; }
    QPushButton *removeButton() const { return m_remove; }
    QPushButton *continueButton() const { return m_continue; }
    QPushButton *pauseButton() const { return m_pause; }
    QPushButton *stepOverButton() const { return m_next; }
    QListWidget *breakpoints() const { return m_breakpoints; }
    QTreeWidget *stack() const { return m_stack; }
    QTreeWidget *registers() const { return m_registers; }
    QTreeWidget *locals() const { return m_locals; }
    // The project whose breakpoints are kept (breakpoints.get/set): the one shown.
    void setProject(const QString &project);
    // Its breakpoints: while debugging, the ones set; else the ones kept for the next run.
    QStringList breakpointLocations() const { return m_kept; }
    // Add a breakpoint at `location`, or remove it if there is one. While paused it is
    // set at once; while not debugging it is kept for the next run; while running
    // GDB cannot take it, and this says so and returns false.
    bool toggleBreakpoint(const QString &location);
    // Run to a function or file:line (debug.control's runTo) while paused; else say
    // why not and return false.
    bool runTo(const QString &location);
    // Expressions read again at every stop (debug.evaluate), kept while the app runs.
    QLineEdit *watchInput() const { return m_watchInput; }
    QTreeWidget *watches() const { return m_watches; }
    void addWatch(const QString &expression);
    // Stop when a watched expression changes (a watchpoint), while paused.
    bool watchChanges(const QString &expression);
    // Look in stack frame `level` (0: where it stopped): its locals, registers and
    // watches; the line it is at is marked (frameSelected).
    void selectFrame(int level);
    int selectedFrame() const { return m_frame; }
    // The memory view: an address or expression, read again at each stop.
    QLineEdit *memoryInput() const { return m_memoryInput; }
    QString memoryText() const;
    void readMemory();
    // A hex dump as the memory view shows it: address, 16 bytes in hex, and as text.
    static QString hexDump(quint64 address, const QByteArray &bytes);
    // Pass a breakpoint the next `count` times (while paused): for a bug on the 500th
    // loop. False when it cannot be now.
    bool ignoreBreakpoint(int number, int count);
    // Show numbers in locals and watches as hex (a value GDB printed as a whole
    // number) or as GDB printed them. Kept between starts.
    QCheckBox *hexBox() const { return m_hex; }
    static QString shownValue(const QString &value, bool hex);
    // A structure's or array's members, under its row (debug.children); its row is
    // expanded to ask. For tests.
    void expandItem(QTreeWidgetItem *item);
    // Set a variable to a value (debug.assign), while paused; the variables are read
    // again after. Double-click a value to be asked.
    bool assign(const QString &expression, const QString &value);
    // Stop watching it (kept for the project too).
    void removeWatch(const QString &expression);
    QStringList watchExpressions() const;
    // Ask for the condition of the breakpoint at `location` (added if there is none):
    // it stops there only when the condition holds. An empty one removes it.
    void editCondition(const QString &location);
    // The same without asking, for tests.
    bool setCondition(const QString &location, const QString &condition);
    // A kept breakpoint's location and condition ("blink.ino:12 if i == 10").
    static QString keptLocation(const QString &kept);
    static QString keptCondition(const QString &kept);

signals:
    void breakpointsChanged();
    // The frame looked in: where to mark the paused line.
    void frameSelected(const QJsonObject &frame);

private:
    enum class Mode { Off, Running, Paused };
    void setMode(Mode mode);
    void addBreakpoint();
    void stopped(const QString &reason, const QJsonObject &frame, int breakpoint, const QJsonObject &watch, int hits);
    void showInspection(const QJsonArray &frames, const QJsonArray &registers, const QJsonArray &locals, int frame);
    // Bold a value that changed since the last stop (frame 0 only); `last` keeps them.
    // Put a value in a cell, as the Hex switch says, keeping GDB's text.
    void setValue(QTreeWidgetItem *item, int column, const QString &value);
    void reformatValues();
    void showChildren(const QString &expression, const QJsonArray &children);
    void editValue(QTreeWidgetItem *item, int column);
    void markChanged(QTreeWidgetItem *item, int column, const QString &key, QHash<QString, QString> &last);
    void keep(const QStringList &locations);
    void evaluateWatches();
    void keepWatches();
    static QString where(const QJsonObject &frame);

    CoreConnection *m_core;
    QLabel *m_status;
    QPushButton *m_continue;
    QPushButton *m_pause;
    QPushButton *m_next;
    QPushButton *m_step;
    QPushButton *m_finish;
    QLineEdit *m_location;
    QPushButton *m_add;
    QPushButton *m_remove;
    QListWidget *m_breakpoints;
    QTreeWidget *m_stack;
    QTreeWidget *m_registers;
    QTreeWidget *m_locals;
    QLineEdit *m_watchInput;
    QTreeWidget *m_watches;
    QCheckBox *m_hex;
    QLineEdit *m_memoryInput;
    QPlainTextEdit *m_memory;
    int m_frame = 0;
    QJsonArray m_frames;
    // Values at the last stop, to show what changed: locals, registers, watches.
    QHash<QString, QString> m_lastLocals;
    QHash<QString, QString> m_lastRegisters;
    QHash<QString, QString> m_lastWatches;
    Mode m_mode = Mode::Off;
    QString m_project;
    // The kept breakpoints, and how many of them are still being set at a run's start.
    QStringList m_kept;
    int m_restoring = 0;
};
