// The debugger: run controls, breakpoints, call stack and registers for a build
// running in the simulator under GDB. The core runs GDB; this renders what it reports.

#pragma once

#include <QJsonArray>
#include <QJsonObject>
#include <QWidget>

class CoreConnection;
class QLabel;
class QLineEdit;
class QListWidget;
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

signals:
    void breakpointsChanged();

private:
    enum class Mode { Off, Running, Paused };
    void setMode(Mode mode);
    void addBreakpoint();
    void stopped(const QString &reason, const QJsonObject &frame, int breakpoint);
    void showInspection(const QJsonArray &frames, const QJsonArray &registers, const QJsonArray &locals);
    void keep(const QStringList &locations);
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
    Mode m_mode = Mode::Off;
    QString m_project;
    // The kept breakpoints, and how many of them are still being set at a run's start.
    QStringList m_kept;
    int m_restoring = 0;
};
