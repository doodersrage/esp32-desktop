// The running or last build: its plan, the arduino-cli log and the outcome, and the
// actions on the last successful build (run, debug, flash), which the main window
// puts in its toolbar with the port picker. Renders only; the core runs the build.

#pragma once

#include <QJsonObject>
#include <QWidget>

class CoreConnection;
class QLabel;
class QPlainTextEdit;
class QComboBox;
class QProgressBar;
class QAction;
class QPushButton;

class BuildPanel : public QWidget {
    Q_OBJECT

public:
    explicit BuildPanel(CoreConnection *core, QWidget *parent = nullptr);

    QString statusText() const;
    QString logText() const;
    // Not in this panel's layout: the main window places it in its toolbar.
    QComboBox *portPicker() const { return m_port; }
    QAction *flashAction() const { return m_flash; }
    QAction *simulateAction() const { return m_simulate; }
    QAction *debugAction() const { return m_debug; }
    // The last build that succeeded.
    QString builtProject() const { return m_builtProject; }
    // From build.status: this project's build for this board is ready to use.
    void setBuildCurrent(const QString &project, const QString &board, bool current);
    QString builtBoard() const { return m_builtBoard; }
    // What Run, Debug and Flash act on: the project and board shown (the board id the
    // build uses), and whether it can be built now. When its last build is not this
    // one, they build it first and then go on. Without a project shown they use the
    // last build.
    void setTarget(const QString &project, const QString &board, bool canBuild);
    // The build that was to be followed by a run or flash was declined.
    void cancelPending();
    // What Run, Debug and Flash act on now.
    QString targetProject() const;
    QString targetBoard() const;

signals:
    // Run, Debug or Flash needs the project shown built first.
    void buildRequested();
    // The actions' state changed: a build finished, ports came or went.
    void actionsChanged();
    // Flash the last successful build to `port`.
    void flashRequested(const QString &port, const QString &project, const QString &board);

private:
    enum class Then { Nothing, Run, Debug, Flash };
    void act(Then what);
    bool targetBuilt() const;
    void started(const QJsonObject &plan);
    void appendLine(const QString &stream, const QString &line);
    void finished(bool ok, const QString &error, const QString &image);
    void setPorts(const QJsonArray &ports);
    void updateFlashButton();

    CoreConnection *m_core;
    QLabel *m_status;
    QPushButton *m_cancel;
    QPlainTextEdit *m_log;
    QComboBox *m_port;
    QAction *m_flash;
    QAction *m_simulate;
    QAction *m_debug;
    QProgressBar *m_progress;
    // The last build that succeeded, which Flash writes.
    QString m_builtProject;
    QString m_currentProject; // built before, and current (build.status)
    QString m_currentBoard;
    QString m_builtBoard;
    QString m_pendingProject;
    QString m_pendingBoard;
    QString m_targetProject;
    QString m_targetBoard;
    bool m_canBuild = false;
    Then m_then = Then::Nothing;
    // The ports in the last list, to pick a board that was just plugged in.
    QStringList m_knownPorts;
};
