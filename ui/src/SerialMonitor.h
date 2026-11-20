// Serial monitor: pick a port and baud rate, see what the board prints, send it
// lines, reset it. The core owns the port; this only renders bytes and sends input.

#pragma once

#include <QHash>
#include <QWidget>

class ConsoleView;
class ConsoleBar;
class CoreConnection;
class QJsonArray;
class QComboBox;
class QLineEdit;
class QPlainTextEdit;
class QPushButton;

class SerialMonitor : public QWidget {
    Q_OBJECT

public:
    ConsoleBar *consoleBar() const { return m_tools; }
    ConsoleView *console() const { return m_output; }
    explicit SerialMonitor(CoreConnection *core, QWidget *parent = nullptr);

    // Open `path` (e.g. from a double-click in the Devices panel).
    void openPort(const QString &path);
    // Choose this baud rate if the port is closed (a project's, say).
    void suggestBaud(int baud);
    // The baud rate a library project's firmware uses (from library.get), or 0.
    int projectBaud(const QString &project) const { return m_projectBauds.value(project); }

    // For tests.
    QString outputText() const;
    QComboBox *portPicker() const { return m_port; }
    QComboBox *baudPicker() const { return m_baud; }
    QLineEdit *input() const { return m_input; }
    QPushButton *openButton() const { return m_open; }

private:
    // Item data role holding a port's USB bridge type from device.list.
    static constexpr int BridgeRole = Qt::UserRole + 1;

    void setPorts(const QJsonArray &ports);
    void send();
    void setOpen(bool open);

    CoreConnection *m_core;
    QComboBox *m_port;
    QComboBox *m_baud;
    QPushButton *m_open;
    QPushButton *m_reset;
    QPushButton *m_clear;
    ConsoleView *m_output;
    QLineEdit *m_input;
    QComboBox *m_ending;
    QPushButton *m_send;
    bool m_open_ = false;
    ConsoleBar *m_tools = nullptr;
    QHash<QString, int> m_projectBauds;
};
