// The running or last build: its plan, the arduino-cli log and the outcome.
// Renders only; the core runs the build.

#pragma once

#include <QJsonObject>
#include <QWidget>

class CoreConnection;
class QLabel;
class QPlainTextEdit;
class QComboBox;
class QProgressBar;
class QPushButton;

class BuildPanel : public QWidget {
    Q_OBJECT

public:
    explicit BuildPanel(CoreConnection *core, QWidget *parent = nullptr);

    QString statusText() const;
    QString logText() const;
    QComboBox *portPicker() const { return m_port; }
    QPushButton *flashButton() const { return m_flash; }

signals:
    // Flash the last successful build to `port`.
    void flashRequested(const QString &port, const QString &project, const QString &board);

private:
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
    QPushButton *m_flash;
    QProgressBar *m_progress;
    // The last build that succeeded, which Flash writes.
    QString m_builtProject;
    QString m_builtBoard;
    QString m_pendingProject;
    QString m_pendingBoard;
};
