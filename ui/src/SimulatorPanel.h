// The simulator: a build running in QEMU, with its UART0 console, input, reset and
// stop. The core runs QEMU; this renders the console and sends input.

#pragma once

#include <QWidget>

class ConsoleView;
class ConsoleBar;
class CoreConnection;
class QCheckBox;
class QComboBox;
class QLabel;
class QLineEdit;
class QPushButton;

class SimulatorPanel : public QWidget {
    Q_OBJECT

public:
    ConsoleBar *consoleBar() const { return m_tools; }
    ConsoleView *console() const { return m_output; }
    QString expectText() const;
    static QString readablePattern(const QString &pattern);
    explicit SimulatorPanel(CoreConnection *core, QWidget *parent = nullptr);

    QString outputText() const;
    QString statusText() const;
    // How long the firmware has run in simulated time (sim.clock), or empty.
    QString clockText() const;
    QString networkText() const;
    QLineEdit *input() const { return m_input; }
    QPushButton *stopButton() const { return m_stop; }
    QPushButton *resetButton() const { return m_reset; }
    QPushButton *runAgainButton() const { return m_again; }
    QCheckBox *icountBox() const { return m_icount; }

private:
    void send();
    void setRunning(bool running);

    CoreConnection *m_core;
    QLabel *m_status;
    QLabel *m_clock;
    // The simulated Wi-Fi network and the link to the sketch's web page.
    QLabel *m_network;
    QLabel *m_expect;
    QPushButton *m_reset;
    QPushButton *m_stop;
    QPushButton *m_again;
    // The last build run here, for Run again.
    QString m_lastProject;
    QString m_lastBoard;
    QPushButton *m_clear;
    // Chip-like timing (sim.start's icount), kept in QSettings; applies to the next run.
    QCheckBox *m_icount;
    ConsoleView *m_output;
    QLineEdit *m_input;
    QComboBox *m_ending;
    QPushButton *m_send;
    ConsoleBar *m_tools = nullptr;
};
