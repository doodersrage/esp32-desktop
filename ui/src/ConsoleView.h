// Read-only text view for serial and simulator output, shared by the serial monitor
// and the simulator panel, and the tool row under it (find, time stamps, save).

#pragma once

#include <QJsonArray>
#include <QJsonValue>
#include <QPlainTextEdit>
#include <QStringDecoder>
#include <QTextBlock>

class CoreConnection;
class QCheckBox;
class QComboBox;
class QLineEdit;
class QToolButton;

// Lines of output by what they are, as the core says (serial.data's, sim.output's
// and build.log's levels): kept as each text block's user state, coloured, and
// shown or hidden by a filter.
namespace OutputLines {
enum State { Plain = 0, Error, Warning, Info, Debug, Verbose, Notice };
int state(const QString &level);
// The colour for a state, from the palette: the text colour for plain and info
// lines, a red and an amber that read on this palette's background (light or dark)
// for errors and warnings, and the dimmed text colour for debug and verbose lines.
QColor colour(int state, const QPalette &palette);
// Set a finished line's state and tag, and colour it.
void mark(QTextBlock block, int state, const QString &tag, const QPalette &palette);
QString tag(const QTextBlock &block);
}

class ConsoleView : public QPlainTextEdit {
    Q_OBJECT

public:
    explicit ConsoleView(QWidget *parent = nullptr);

    // Which lines to show: all, warnings and errors (and the app's notices), or
    // errors only; and only lines with this tag, unless it is empty.
    enum LevelFilter { AllLines, Problems, ErrorsOnly };
    void setLevelFilter(LevelFilter filter);
    void setTagFilter(const QString &tag);
    LevelFilter levelFilter() const { return m_levelFilter; }
    // The tags seen so far, in the order they first came.
    QStringList tags() const { return m_tags; }
    // The lines shown, for tests.
    QStringList visibleLines() const;

    // A chunk of output: as the core shows it (`shown`, without colour codes) with
    // the levels and tags of the lines it finished, or, in hex or from a core that
    // does not send `shown`, the raw bytes.
    void appendOutput(const QByteArray &bytes, const QJsonValue &shown, const QJsonArray &lines);

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
    // Set the steps from the system's size (Preferences), without saving them.
    void setZoomSteps(int steps);
    int zoomSteps() const { return m_zoom; }
    // Scrolled up while output came: the "New output" button that goes to the end shows.
    QToolButton *newOutputButton() const { return m_jump; }

signals:
    void tagsChanged();

protected:
    void wheelEvent(QWheelEvent *event) override;
    void changeEvent(QEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;

private:
    // After adding text: follow it when the view was at the end, else offer to.
    void followOrOffer(bool wasAtEnd);
    void placeJump();
    QToolButton *m_jump = nullptr;
    void applyZoom();
    void appendText(QString text);
    void markLines(const QJsonArray &lines);
    bool shows(const QTextBlock &block) const;
    void refilter();
    void recolour();

    LevelFilter m_levelFilter = AllLines;
    QString m_tagFilter;
    QStringList m_tags;

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
    QComboBox *levelPicker() const { return m_levels; }
    QComboBox *tagPicker() const { return m_tags; }
    // Put the cursor in the find field, its text selected (Ctrl+F).
    void focusFind();
    QCheckBox *timestampBox() const { return m_stamps; }
    // Ask the core to write the console's text to `path` (Save… asks for it).
    void saveTo(const QString &path);

private:
    ConsoleView *m_view;
    CoreConnection *m_core;
    QLineEdit *m_find;
    QComboBox *m_levels;
    QComboBox *m_tags;
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
