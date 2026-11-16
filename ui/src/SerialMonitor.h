// Serial monitor: pick a port and baud rate, see what the board prints, send it
// lines, reset it. The core owns the port; this only renders bytes and sends input.

#pragma once

#include <QStringDecoder>
#include <QWidget>

class CoreConnection;
class QJsonArray;
class QComboBox;
class QLineEdit;
class QPlainTextEdit;
class QPushButton;

class SerialMonitor : public QWidget {
    Q_OBJECT

public:
    explicit SerialMonitor(CoreConnection *core, QWidget *parent = nullptr);

    // Open `path` (e.g. from a double-click in the Devices panel).
    void openPort(const QString &path);

    // For tests.
    QString outputText() const;
    QComboBox *portPicker() const { return m_port; }
    QComboBox *baudPicker() const { return m_baud; }
    QLineEdit *input() const { return m_input; }
    QPushButton *openButton() const { return m_open; }

private:
    void setPorts(const QJsonArray &ports);
    void appendBytes(const QByteArray &bytes);
    void appendNotice(const QString &text);
    void send();
    void setOpen(bool open);
    void suggestBaud(int baud);

    CoreConnection *m_core;
    QComboBox *m_port;
    QComboBox *m_baud;
    QPushButton *m_open;
    QPushButton *m_reset;
    QPushButton *m_clear;
    QPlainTextEdit *m_output;
    QLineEdit *m_input;
    QComboBox *m_ending;
    QPushButton *m_send;
    // Stateful, so a character split across two chunks still decodes.
    QStringDecoder m_decoder{QStringDecoder::Utf8};
    bool m_open_ = false;
};
