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
    // Render a library.get result. Public so tests can feed fixed data.
    void showResult(const QJsonObject &result);

    // For tests.
    QString title() const;
    QComboBox *boardPicker() const { return m_board; }
    QTreeWidget *partsTree() const { return m_parts; }
    QTreeWidget *wiringTree() const { return m_wiring; }
    QPlainTextEdit *codeView() const { return m_code; }
    QTabWidget *tabs() const { return m_tabs; }
    QString notesText() const;

private:
    void showBuild(int index);
    void showPlaceholder(const QString &text);

    CoreConnection *m_core;
    QString m_requestedId;
    QJsonObject m_project;
    QJsonArray m_builds;

    QStackedWidget *m_stack;
    QLabel *m_placeholder;
    QLabel *m_title;
    QLabel *m_meta;
    QLabel *m_summary;
    QPushButton *m_openSite;
    QComboBox *m_board;
    QTabWidget *m_tabs;
    QTreeWidget *m_parts;
    QLabel *m_libraries;
    QTreeWidget *m_wiring;
    QLabel *m_pins;
    QPlainTextEdit *m_code;
    QPlainTextEdit *m_output;
    QLabel *m_notes;
};
