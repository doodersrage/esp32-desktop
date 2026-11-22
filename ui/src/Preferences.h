// The app's preferences, kept with QSettings, and the dialog that changes them
// (Tools → Preferences…). They are the app's own; the core keeps nothing of them.

#pragma once

#include <QDialog>

class QCheckBox;
class QComboBox;
class QSpinBox;

namespace Preferences {
// The language chosen ("" for the desktop's, "en", or one of Translations::languages()).
QString language();
// The colours chosen: "" for the system's, "light" or "dark".
QString theme();
// Apply theme() to the app (Qt 6.8 and later; earlier, the system's stays).
void applyTheme();
// Whether this Qt can choose the colours at all.
bool canChooseTheme();
// Whether to say when a newer release is out (library.list's update).
bool releaseNotices();
// Whether to reopen the last project and board at start.
bool reopenProject();
// Whether the serial monitor waits for a board that goes away (serial.open's reconnect).
bool serialReconnect();
// Whether to reopen the serial monitor's port at start when it was open at quit.
bool reopenSerial();
// The interface size chosen, in percent (100 is the desktop's own scaling).
int interfaceSize();
// Before the QApplication exists: ask Qt for interfaceSize() (QT_SCALE_FACTOR),
// unless the environment already sets a scale.
void applyInterfaceSize();
}

class PreferencesDialog : public QDialog {
    Q_OBJECT

public:
    explicit PreferencesDialog(QWidget *parent = nullptr);

    // Load the saved values into the controls (when the dialog opens).
    void load();
    // Save the controls' values (OK).
    void save();
    QComboBox *languagePicker() const { return m_language; }
    QComboBox *themePicker() const { return m_theme; }
    QSpinBox *textSize() const { return m_textSize; }
    QCheckBox *releaseNoticesBox() const { return m_notices; }
    QComboBox *sizePicker() const { return m_size; }

signals:
    // The consoles' text size changed: they apply it.
    void textSizeChanged(int steps);

private:
    QComboBox *m_language;
    QComboBox *m_theme;
    QComboBox *m_size;
    QSpinBox *m_textSize;
    QCheckBox *m_notices;
    QCheckBox *m_reopenProject;
    QCheckBox *m_reconnect;
    QCheckBox *m_reopenSerial;
};
