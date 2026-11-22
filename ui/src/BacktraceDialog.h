// Decode a crash backtrace pasted from anywhere (a saved log, a forum post) against a
// build of the project shown (backtrace.decode). The core decodes; this shows the
// frames, and a double-click on one shows its line.

#pragma once

#include <QDialog>
#include <QJsonArray>
#include <QJsonObject>

class CoreConnection;
class QLabel;
class QPlainTextEdit;
class QPushButton;
class QTreeWidget;

class BacktraceDialog : public QDialog {
    Q_OBJECT

public:
    explicit BacktraceDialog(CoreConnection *core, QWidget *parent = nullptr);

    // What to decode against: a project and the board its build is for.
    void setBuild(const QString &project, const QString &board, const QString &title);

    QPlainTextEdit *input() const { return m_input; }
    QPushButton *decodeButton() const { return m_decode; }
    QTreeWidget *frames() const { return m_frames; }
    QString statusText() const;

signals:
    // {file (its name), line, path}, as ConsoleView::frameActivated gives it.
    void locationActivated(const QJsonObject &at);

private:
    void showResult(const QJsonArray &backtraces);

    CoreConnection *m_core;
    QString m_project;
    QString m_board;
    QLabel *m_about;
    QPlainTextEdit *m_input;
    QPushButton *m_decode;
    QLabel *m_status;
    QTreeWidget *m_frames;
    bool m_waiting = false;
};
