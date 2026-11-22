// Serial monitor: pick a port and baud rate, see what the board prints, send it
// lines, reset it. The core owns the port; this only renders bytes and sends input.

#pragma once

#include <QHash>
#include <QWidget>

class ConsoleView;
class ConsoleBar;
class CoreConnection;
class QJsonArray;
class QCheckBox;
class QHBoxLayout;
class QLabel;
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
    // With `expect`, a library project: say whether the board prints what it expects
    // (after flashing it); with `board` too, decode its crash backtraces from that build.
    void openPort(const QString &path, const QString &expect = {}, const QString &board = {});
    QString expectText() const;
    QPushButton *garbledSwitch() const { return m_garbledSwitch; }
    // Record…: everything the board sends, appended to a file (serial.record).
    QPushButton *recordButton() const { return m_record; }
    // Start recording to `path`, or stop with an empty one; Record… asks for the file.
    void recordTo(const QString &path);
    // At start: choose the port used last time when it shows up, and open it again
    // if it was open when the app quit (Preferences::reopenSerial).
    void restoreLastPort() { m_restore = true; }
    // The project shown, whose quick-send lines the monitor offers (commands.get).
    void setProject(const QString &project);
    QStringList quickCommands() const { return m_commands; }
    // Keep the line typed as a quick-send button; send a kept line; forget one.
    void keepInput();
    void sendCommand(const QString &line);
    void forgetCommand(const QString &line);
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
    QCheckBox *hexBox() const { return m_hex; }
    // Typed hex ("01 a0 ff", "0x01a0ff") as bytes; empty if it is not hex.
    static QByteArray hexBytes(const QString &text);

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
    QPushButton *m_record = nullptr;
    QString m_recordingTo;
    ConsoleView *m_output;
    QLineEdit *m_input;
    QComboBox *m_ending;
    QPushButton *m_send;
    QCheckBox *m_hex;
    QLabel *m_expect;
    QLabel *m_garbled = nullptr;
    QPushButton *m_garbledSwitch = nullptr;
    QString m_project;
    QStringList m_commands;
    QHBoxLayout *m_quick = nullptr;
    QPushButton *m_keep = nullptr;
    void showCommands();
    bool m_restore = false;  // restoreLastPort() was called
    bool m_restored = false; // the last session's port chosen (or not there)
    bool m_open_ = false;
    ConsoleBar *m_tools = nullptr;
    QHash<QString, int> m_projectBauds;
};
