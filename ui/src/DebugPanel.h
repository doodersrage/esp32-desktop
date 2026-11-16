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

private:
    enum class Mode { Off, Running, Paused };
    void setMode(Mode mode);
    void addBreakpoint();
    void stopped(const QString &reason, const QJsonObject &frame, int breakpoint);
    void showInspection(const QJsonArray &frames, const QJsonArray &registers);
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
    Mode m_mode = Mode::Off;
};
