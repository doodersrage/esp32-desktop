// The running or last build: its plan, the arduino-cli log and the outcome.
// Renders only; the core runs the build.

#pragma once

#include <QJsonObject>
#include <QWidget>

class CoreConnection;
class QLabel;
class QPlainTextEdit;
class QPushButton;

class BuildPanel : public QWidget {
    Q_OBJECT

public:
    explicit BuildPanel(CoreConnection *core, QWidget *parent = nullptr);

    QString statusText() const;
    QString logText() const;

private:
    void started(const QJsonObject &plan);
    void appendLine(const QString &stream, const QString &line);
    void finished(bool ok, const QString &error, const QString &image);

    CoreConnection *m_core;
    QLabel *m_status;
    QPushButton *m_cancel;
    QPlainTextEdit *m_log;
};
