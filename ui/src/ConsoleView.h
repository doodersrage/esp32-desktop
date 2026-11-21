// Read-only text view for serial and simulator output, shared by the serial monitor
// and the simulator panel, and the tool row under it (find, time stamps, save).

#pragma once

#include <QPlainTextEdit>
#include <QStringDecoder>

class CoreConnection;
class QCheckBox;
class QLineEdit;
class QToolButton;

class ConsoleView : public QPlainTextEdit {
    Q_OBJECT

public:
    explicit ConsoleView(QWidget *parent = nullptr);

    void appendBytes(const QByteArray &bytes);
    // A line from the app, not the board, marked so it stands apart.
    void appendNotice(const QString &text);
    // Start a new stream: forget any half-decoded character.
    void resetDecoder();
    // Start each new line with this computer's time (hh:mm:ss.zzz). Lines already
    // shown keep what they have.
    void setTimestamps(bool on);
    bool timestamps() const { return m_timestamps; }
    // Select the next (or previous) match after the cursor, wrapping round; false if
    // there is none.
    bool findText(const QString &text, bool backward = false);
    // Make the text bigger (steps > 0) or smaller, or back to the system's size; kept
    // for every console (QSettings console/zoom). Ctrl+= / Ctrl+- / Ctrl+0 and
    // Ctrl+wheel do the same.
    // Show bytes as hex digits (`48 69 0a`), for binary protocols; lines already
    // shown stay as they are.
    void setHex(bool on);
    bool hex() const { return m_hex; }
    static QString hexText(const QByteArray &bytes);
    void zoom(int steps);
    void resetZoom();
    int zoomSteps() const { return m_zoom; }

protected:
    void wheelEvent(QWheelEvent *event) override;

private:
    void applyZoom();

    QStringDecoder m_decoder{QStringDecoder::Utf8};
    int m_zoom = 0;
    bool m_hex = false;
    bool m_timestamps = false;
    bool m_atLineStart = true;
};

// Find, time stamps and Save… for a ConsoleView. Saving goes through the core
// (console.save), which writes the file.
class ConsoleBar : public QWidget {
    Q_OBJECT

public:
    // `setting` names where the time-stamps choice is kept (QSettings).
    ConsoleBar(ConsoleView *view, CoreConnection *core, const QString &setting, QWidget *parent = nullptr);

    QLineEdit *findField() const { return m_find; }
    // Put the cursor in the find field, its text selected (Ctrl+F).
    void focusFind();
    QCheckBox *timestampBox() const { return m_stamps; }
    // Ask the core to write the console's text to `path` (Save… asks for it).
    void saveTo(const QString &path);

private:
    ConsoleView *m_view;
    CoreConnection *m_core;
    QLineEdit *m_find;
    QToolButton *m_prev;
    QToolButton *m_next;
    QCheckBox *m_stamps;
    QToolButton *m_save;
};

// Up and Down in a console's input line recall the lines sent before, newest first,
// as in the Arduino IDE's serial monitor. Kept while the app runs.
class InputHistory : public QObject {
    Q_OBJECT

public:
    // Watches `input`; lines are remembered when Return sends them.
    explicit InputHistory(QLineEdit *input);
    QStringList lines() const { return m_lines; }

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    QLineEdit *m_input;
    QStringList m_lines; // oldest first
    int m_at = -1;       // index being shown, -1 when editing a new line
    QString m_draft;     // what was typed before going back
};
