// The simulator: a build running in QEMU, with its UART0 console, input, reset and
// stop. The core runs QEMU; this renders the console and sends input.

#pragma once

#include <QWidget>

class ConsoleView;
class CoreConnection;
class QComboBox;
class QLabel;
class QLineEdit;
class QPushButton;

class SimulatorPanel : public QWidget {
    Q_OBJECT

public:
    explicit SimulatorPanel(CoreConnection *core, QWidget *parent = nullptr);

    QString outputText() const;
    QString statusText() const;
    QLineEdit *input() const { return m_input; }
    QPushButton *stopButton() const { return m_stop; }
    QPushButton *resetButton() const { return m_reset; }
    QPushButton *runAgainButton() const { return m_again; }

private:
    void send();
    void setRunning(bool running);

    CoreConnection *m_core;
    QLabel *m_status;
    QPushButton *m_reset;
    QPushButton *m_stop;
    QPushButton *m_again;
    // The last build run here, for Run again.
    QString m_lastProject;
    QString m_lastBoard;
    QPushButton *m_clear;
    ConsoleView *m_output;
    QLineEdit *m_input;
    QComboBox *m_ending;
    QPushButton *m_send;
};
