// The running or last build: its plan, the arduino-cli log and the outcome, and the
// actions on the last successful build (run, debug, flash), which the main window
// puts in its toolbar with the port picker. Renders only; the core runs the build.

#pragma once

#include <QJsonObject>
#include <QElapsedTimer>
#include <QWidget>

class CoreConnection;
class QCheckBox;
class QLabel;
class QPlainTextEdit;
class QComboBox;
class QProgressBar;
class QAction;
class QPushButton;
class QToolButton;

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
    // F8 / Shift+F8: the build's next (or previous) error or warning that points at a
    // line, shown as a double-click on it would; round from the end.
    QAction *nextProblemAction() const { return m_nextProblem; }
    QAction *previousProblemAction() const { return m_previousProblem; }
    bool goToProblem(bool forward);
    // The last build that succeeded.
    QString builtProject() const { return m_builtProject; }
    // From build.status: this project's build for this board is ready to use.
    void setBuildCurrent(const QString &project, const QString &board, bool current);
    // For tests: the error and warning counts, the filter, and the lines it shows.
    QString problemText() const;
    QCheckBox *problemsOnlyBox() const { return m_problemsOnly; }
    QStringList visibleLogLines() const;
    // Whether that project is built for that board, this session or before (current).
    bool isBuilt(const QString &project, const QString &board) const;
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
    // Cancel the build that is running, if there is one (Esc in this dock).
    bool cancelRunning();
    // The error and warning lines, for pasting into a forum post or an issue.
    QString problemLines() const;
    QPushButton *copyProblemsButton() const { return m_copyProblems; }
    // The problem lines, else the log's last 200 lines, as a forum post's code block
    // naming the app, the project and the board (Copy for a forum post).
    QString forumText() const;
    QToolButton *forumButton() const { return m_forum; }
    // Show what a located log line points at, as a double-click on it does. For tests.
    bool activateLine(int blockNumber);

signals:
    // Run, Debug or Flash needs the project shown built first.
    void buildRequested();
    // The actions' state changed: a build finished, ports came or went.
    void actionsChanged();
    // Flash the last successful build to `port`.
    void flashRequested(const QString &port, const QString &project, const QString &board);
    // A double-click or Enter on an error or warning line that names a file and line:
    // build.log's location (file, line, column, and path for a local project's file).
    void locationActivated(const QJsonObject &location);

private:
    enum class Then { Nothing, Run, Debug, Flash };
    void act(Then what);
    bool targetBuilt() const;
    void started(const QJsonObject &plan);
    void appendLine(const QString &stream, const QString &line, const QString &level = {},
                    const QJsonObject &location = {});
    bool eventFilter(QObject *watched, QEvent *event) override;
    void finished(bool ok, const QString &error, const QString &image);
    void setPorts(const QJsonArray &ports);
    void updateFlashButton();

    CoreConnection *m_core;
    QLabel *m_status;
    QPushButton *m_cancel;
    QPlainTextEdit *m_log;
    QToolButton *m_forum = nullptr;
    QPushButton *m_copyProblems = nullptr;
    QLabel *m_problems;
    QCheckBox *m_problemsOnly;
    int m_errors = 0;
    int m_warnings = 0;
    void showProblemCount();
    QElapsedTimer m_flashClock;
    QJsonObject m_size; // the build's size, until it has finished
    double m_seconds = -1; // how long it took (build.finished's seconds), or -1
    qint64 m_flashFrom = 0;
public:
    // The flash progress bar's words: percent, then speed and time left once known.
    QString flashProgressText(qint64 written, qint64 total);
private:
    QComboBox *m_port;
    QAction *m_flash;
    QAction *m_simulate;
    QAction *m_debug;
    QAction *m_nextProblem;
    QAction *m_previousProblem;
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
