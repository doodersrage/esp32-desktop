// Read-only console for a byte stream (serial port or simulator UART): decodes UTF-8
// with state, so characters split across chunks survive; drops CR; keeps notices on
// their own line; scrolls only when already at the end.

#pragma once

#include <QPlainTextEdit>
#include <QStringDecoder>

class ConsoleView : public QPlainTextEdit {
    Q_OBJECT

public:
    explicit ConsoleView(QWidget *parent = nullptr);

    void appendBytes(const QByteArray &bytes);
    // A line from the app, not the board, marked so it stands apart.
    void appendNotice(const QString &text);
    // Start a new stream: forget any half-decoded character.
    void resetDecoder();

private:
    QStringDecoder m_decoder{QStringDecoder::Utf8};
};
