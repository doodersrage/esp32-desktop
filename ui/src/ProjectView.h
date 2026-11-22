// One project from the library (library.get): summary, and per board the parts,
// wiring, code and expected output. Renders only.

#pragma once

#include <QJsonArray>
#include <QJsonObject>
#include <QHash>
#include <QWidget>

#include <utility>

class CoreConnection;
class QComboBox;
class QLabel;
class QPlainTextEdit;
class QAction;
class QPushButton;
class QToolButton;
class QStackedWidget;
class CodeView;
class QTabWidget;
class QTreeWidget;

class ProjectView : public QWidget {
    Q_OBJECT

public:
    enum Tab { PartsTab, WiringTab, CodeTab, OutputTab, NotesTab };

    explicit ProjectView(CoreConnection *core, QWidget *parent = nullptr);

    // Ask the core for a project; the view updates when the answer comes.
    void showProject(const QString &id);
    // Show `board` when the project next arrives, if it has a build for it.
    void preferBoard(const QString &board) { m_wantBoard = board; }
    // Chips of the connected boards that say theirs (device.list): a library project
    // starts on a build for one, and switches to it when one is plugged in, unless a
    // board was chosen.
    void setConnectedChips(const QStringList &chips);
    // Whether the guide offers to try Blink (the library has it).
    void setTryAvailable(bool available);
    QPushButton *tryButton() const { return m_try; }
    QLabel *placeholderLabel() const { return m_placeholder; }
    // Render a library.get result. Public so tests can feed fixed data.
    void showResult(const QJsonObject &result);
    // Render a local.get result: one of the user's ESP-IDF projects.
    void showLocal(const QJsonObject &result);

    // For tests.
    QString title() const;
    QComboBox *boardPicker() const { return m_board; }
    QTreeWidget *partsTree() const { return m_parts; }
    QTreeWidget *wiringTree() const { return m_wiring; }
    CodeView *codeView() const { return m_code; }
    QTabWidget *tabs() const { return m_tabs; }
    QPushButton *buildButton() const { return m_build; }
    QPushButton *copyCodeButton() const { return m_copyCode; }
    QPushButton *saveCodeButton() const { return m_saveCode; }
    QPushButton *openIdeButton() const { return m_openIde; }
    QString buildStateText() const;
    // Show a line of the library sketch shown (1-based), selected in the Code tab, as
    // a build error points at it; false when no sketch is shown or it is shorter.
    bool showCodeLine(int line);
    // The line the debugger is paused at in the sketch shown (0 for none): marked, and
    // the Code tab shown when there is one.
    void showPausedLine(int line);
    // Render a build.status result for the project and board shown. Public for tests.
    void showBuildState(const QJsonObject &status);
    // The same as the Build button, for the main window's toolbar: enabled when the
    // project shown can be built now.
    QAction *buildAction() const { return m_buildAction; }
    // What is shown, for the toolbar: the project's title and the board's name, or
    // empty when no project is shown.
    QString currentProjectTitle() const;
    // The project's id and the board id a build of it uses, or empty.
    QString currentProjectId() const;
    QString currentBuildBoard() const;
    QString currentBoardName() const;
    QPushButton *openButton() const { return m_openSite; }
    QPushButton *copyLinkButton() const { return m_copyLink; }
    // A local project's source files, to open in the user's editor.
    QToolButton *editButton() const { return m_edit; }
    QString summaryText() const;
    QString notesText() const;

signals:
    // A click beside line `line` of the sketch shown (the Code tab's gutter).
    void codeLineClicked(int line);
    // The project and board now shown, with the build's pin list as the library has it.
    void boardSelected(const QString &project, const QString &board, const QJsonArray &pins);
    // What is shown or whether it can be built changed (currentProjectTitle(), buildAction()).
    void currentChanged();
    // The guide's Try Blink button.
    void tryRequested();
    // Open in Arduino IDE was pressed for this project and board.
    void openInIdeRequested(const QString &project, const QString &board);
    // A recent project on the guide was clicked.
    void recentRequested(const QString &id);
    // build.status said whether this project's build for this board can be used as it
    // is (true), or must be built first.
    void buildCurrent(const QString &project, const QString &board, bool current);

public:
    // Nothing selected yet: how to get from a project to a running board.
    void showGuide();
    // The last projects shown (title, id), newest first, listed on the guide.
    void setRecent(const QList<std::pair<QString, QString>> &recent);
    QString recentHtml() const;
    // What each pin of `board` is wired to, if `project` is the one shown.
    QHash<int, QString> pinLabels(const QString &project, const QString &board) const;

private:
    QList<std::pair<QString, QString>> m_recent;
    QJsonObject m_pinLabels;
    void showBuild(int index);
    void requestBuildState();
    // The chip id of `board` in a library.get result.
    static QString chipOf(const QJsonObject &result, const QString &board);
    // The index of a build for a connected board's chip, or -1.
    int buildForConnectedChip() const;
    void showPlaceholder(const QString &text);
    void setBuildToolTip(const QString &text);
    void updateBuildButton();

    CoreConnection *m_core;
    QString m_requestedId;
    QString m_wantBoard;
    QJsonObject m_project;
    QJsonArray m_builds;
    // A local ESP-IDF project, and whether it can be built now (ESP-IDF found, folder there).
    bool m_local = false;
    bool m_localBuildable = false;
    // The board shown was chosen (by hand, by a link, or kept): plugging in a board
    // does not change it.
    bool m_boardChosen = false;
    QStringList m_connectedChips;
    QPushButton *m_try = nullptr;
    bool m_tryAvailable = false;
    static constexpr int ChipRole = Qt::UserRole + 1;
    // A board item's name without "· connected".
    static constexpr int NameRole = Qt::UserRole + 2;
    void markConnectedBoards();

    QStackedWidget *m_stack;
    QLabel *m_placeholder;
    QLabel *m_title;
    QLabel *m_meta;
    QLabel *m_summary;
    QPushButton *m_openSite;
    QPushButton *m_copyLink = nullptr;
    QToolButton *m_edit;
    QPushButton *m_build;
    QAction *m_buildAction;
    QComboBox *m_board;
    QLabel *m_boardLabel;
    QTabWidget *m_tabs;
    QTreeWidget *m_parts;
    QLabel *m_libraries;
    QTreeWidget *m_wiring;
    QLabel *m_pins;
    CodeView *m_code;
    QPushButton *m_copyCode;
    QPushButton *m_saveCode;
    QPushButton *m_openIde;
    QLabel *m_buildState;
    QPlainTextEdit *m_output;
    QLabel *m_notes;
};
