// One project from the library (library.get): summary, and per board the parts,
// wiring, code and expected output. Renders only.

#pragma once

#include <QJsonArray>
#include <QJsonObject>
#include <QWidget>

class CoreConnection;
class QComboBox;
class QLabel;
class QPlainTextEdit;
class QAction;
class QPushButton;
class QStackedWidget;
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
    // Render a library.get result. Public so tests can feed fixed data.
    void showResult(const QJsonObject &result);
    // Render a local.get result: one of the user's ESP-IDF projects.
    void showLocal(const QJsonObject &result);

    // For tests.
    QString title() const;
    QComboBox *boardPicker() const { return m_board; }
    QTreeWidget *partsTree() const { return m_parts; }
    QTreeWidget *wiringTree() const { return m_wiring; }
    QPlainTextEdit *codeView() const { return m_code; }
    QTabWidget *tabs() const { return m_tabs; }
    QPushButton *buildButton() const { return m_build; }
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
    QString summaryText() const;
    QString notesText() const;

signals:
    // The project and board now shown, with the build's pin list as the library has it.
    void boardSelected(const QString &project, const QString &board, const QJsonArray &pins);
    // What is shown or whether it can be built changed (currentProjectTitle(), buildAction()).
    void currentChanged();

private:
    void showBuild(int index);
    void showPlaceholder(const QString &text);
    // Nothing selected yet: how to get from a project to a running board.
    void showGuide();
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

    QStackedWidget *m_stack;
    QLabel *m_placeholder;
    QLabel *m_title;
    QLabel *m_meta;
    QLabel *m_summary;
    QPushButton *m_openSite;
    QPushButton *m_build;
    QAction *m_buildAction;
    QComboBox *m_board;
    QLabel *m_boardLabel;
    QTabWidget *m_tabs;
    QTreeWidget *m_parts;
    QLabel *m_libraries;
    QTreeWidget *m_wiring;
    QLabel *m_pins;
    QPlainTextEdit *m_code;
    QPlainTextEdit *m_output;
    QLabel *m_notes;
};
