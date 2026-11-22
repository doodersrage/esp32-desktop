#include "SimulatorPanel.h"

#include "ConsoleView.h"
#include "CoreConnection.h"
#include "Labels.h"

#include <QAction>
#include <QCheckBox>
#include <QComboBox>
#include <QSettings>
#include <QHBoxLayout>
#include <QRegularExpression>
#include <QIcon>
#include <QJsonObject>
#include <QKeySequence>
#include <QLabel>
#include <QLocale>
#include <QLineEdit>
#include <QPushButton>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

// A site pattern as a person reads it: "tick \\d+ ms" as "tick … ms".
QString SimulatorPanel::readablePattern(const QString &pattern)
{
    QString t = pattern;
    for (const auto &re : {u"-\\?\\\\d\\+"_s, u"\\\\d\\+"_s, u"\\.\\+"_s, u"\\.\\*"_s})
        t.replace(QRegularExpression(re), u"…"_s);
    t.replace(QRegularExpression(u"\\\\(.)"_s), u"\\1"_s);
    t.replace(u"….…"_s, u"…"_s); // a decimal number
    return t;
}

QString SimulatorPanel::expectText() const
{
    return m_expect->isHidden() ? QString() : m_expect->text();
}

SimulatorPanel::SimulatorPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(tr("Not running. Build a project for an ESP32, ESP32-S3 or ESP32-C3 board, "
                             "then choose Run in simulator."),
                          this))
    , m_network(new QLabel(this))
    , m_expect(new QLabel(this))
    , m_reset(new QPushButton(QIcon::fromTheme(u"view-refresh"_s), tr("Reset"), this))
    , m_pause(new QPushButton(QIcon::fromTheme(u"media-playback-pause"_s), tr("Pause"), this))
    , m_pauseAction(new QAction(QIcon::fromTheme(u"media-playback-pause"_s), tr("Pause simulation"), this))
    , m_stop(new QPushButton(QIcon::fromTheme(u"media-playback-stop"_s), tr("Stop"), this))
    , m_again(new QPushButton(QIcon::fromTheme(u"media-playback-start"_s), tr("Run again"), this))
    , m_clear(new QPushButton(QIcon::fromTheme(u"edit-clear"_s), tr("Clear"), this))
    , m_icount(new QCheckBox(tr("Chip-like timing"), this))
    , m_output(new ConsoleView(this))
    , m_input(new QLineEdit(this))
    , m_ending(new QComboBox(this))
    , m_send(new QPushButton(tr("Send"), this))
{
    m_output->setAccessibleName(tr("Simulator output"));
    m_input->setAccessibleName(tr("Text to send"));
    m_ending->setAccessibleName(tr("Line ending"));
    m_status->setWordWrap(true);
    m_status->setToolTip(tr("The simulator runs the firmware. Parts placed in the Circuit dock run with it; "
                            "there is no Bluetooth."));
    m_network->setWordWrap(true);
    m_network->setTextFormat(Qt::RichText);
    m_network->setOpenExternalLinks(true);
    m_network->setToolTip(tr("A simulated Wi-Fi access point. The firmware gets an address from it "
                             "and nothing else: no internet, no name lookups, no other computers. "
                             "Port 80 of the firmware opens on this computer."));
    m_network->hide();
    m_input->setPlaceholderText(tr("Text to send to the simulated board"));
    m_ending->addItem(tr("LF"), u"\n"_s);
    m_ending->addItem(tr("CR LF"), u"\r\n"_s);
    m_ending->addItem(tr("No line ending"), QString());

    connect(m_reset, &QPushButton::clicked, m_core, &CoreConnection::resetSim);
    m_pauseAction->setShortcut(QKeySequence(Qt::Key_F6));
    m_pauseAction->setShortcutContext(Qt::ApplicationShortcut);
    connect(m_pauseAction, &QAction::triggered, this, [this] {
        if (m_core->isSimRunning() && !m_core->isDebugging())
            m_core->pauseSim(!m_core->isSimPaused());
    });
    connect(m_pause, &QPushButton::clicked, m_pauseAction, &QAction::trigger);
    connect(m_core, &CoreConnection::simPaused, this, [this](bool paused) {
        if (paused) {
            m_status->setText(tr("Paused: the simulated time, pins and trace stand still."));
        } else if (!m_runningText.isEmpty()) {
            m_status->setText(m_runningText);
        }
        updatePause();
    });
    connect(m_core, &CoreConnection::debugStarted, this, &SimulatorPanel::updatePause);
    connect(m_stop, &QPushButton::clicked, m_core, &CoreConnection::stopSim);
    m_again->setToolTip(tr("Run the same build again, without rebuilding"));
    connect(m_again, &QPushButton::clicked, this, [this] {
        if (!m_lastProject.isEmpty() && !m_core->isSimRunning())
            m_core->startSim(m_lastProject, m_lastBoard);
    });
    connect(m_clear, &QPushButton::clicked, m_output, &ConsoleView::clear);
    m_icount->setToolTip(tr("Count time in instructions, as the chip does: timing no longer depends on how "
                            "busy this computer is. Simulations run slower (an ESP32 takes about two seconds "
                            "to boot), and a sketch that waits in a loop reading hardware, such as an SD card "
                            "library looking for a card, can take minutes. Applies from the next run."));
    m_icount->setChecked(QSettings().value(u"simulator/icount"_s, false).toBool());
    m_core->setSimIcount(m_icount->isChecked());
    connect(m_icount, &QCheckBox::toggled, this, [this](bool on) {
        QSettings().setValue(u"simulator/icount"_s, on);
        m_core->setSimIcount(on);
    });
    connect(m_send, &QPushButton::clicked, this, &SimulatorPanel::send);
    new InputHistory(m_input); // Up and Down recall what was sent
    connect(m_input, &QLineEdit::returnPressed, this, &SimulatorPanel::send);

    m_clock = new QLabel(this);
    m_clock->setToolTip(tr("How long the firmware has run, in the simulation's own time. It can fall behind "
                           "the real time, most with Chip-like timing on."));
    m_clock->hide();
    connect(m_core, &CoreConnection::simClock, this, [this](qint64 ns) {
        const qint64 tenths = ns / 100'000'000;
        const QString time = tenths < 600
            ? tr("%1 s").arg(QLocale().toString(tenths / 10.0, 'f', 1))
            : tr("%1 min %2 s").arg(tenths / 600).arg(tenths / 10 % 60, 2, 10, QLatin1Char('0'));
        m_clock->setText(tr("%1 simulated").arg(time));
        m_clock->show();
    });
    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_clock);
    top->addWidget(m_pause);
    top->addWidget(m_reset);
    top->addWidget(m_stop);
    top->addWidget(m_again);
    top->addWidget(m_icount);
    top->addWidget(m_clear);
    auto *bottom = new QHBoxLayout;
    bottom->addWidget(m_input, 1);
    bottom->addWidget(m_ending);
    bottom->addWidget(m_send);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_network);
    layout->addWidget(m_expect);
    layout->addWidget(m_output, 1);
    m_tools = new ConsoleBar(m_output, m_core, u"simulator/timestamps"_s, this);
    layout->addWidget(m_tools);
    layout->addLayout(bottom);

    connect(m_core, &CoreConnection::simStarted, this,
            [this](const QString &project, const QString &board, const QString &chip) {
                m_output->resetDecoder();
                m_output->appendNotice(tr("Started %1 for %2 on a simulated %3.")
                                           .arg(project, board, Labels::chipName(chip)));
                m_runningText = tr("Running %1 on a simulated %2.").arg(project, Labels::chipName(chip));
                m_tools->setContext(tr("%1 for %2, on a simulated %3").arg(project, board, Labels::chipName(chip)));
                m_status->setText(m_runningText);
                m_clock->hide();
                m_lastProject = project;
                m_lastBoard = board;
                m_expect->hide();
                const QJsonObject net = m_core->simNetwork();
                if (net.isEmpty()) {
                    m_network->hide();
                } else {
                    const QString url = net.value(u"webUrl"_s).toString();
                    m_network->setText(tr("Simulated Wi-Fi: %1 (open, no internet). The firmware's web page: "
                                          "<a href=\"%2\">%2</a>")
                                           .arg(net.value(u"ssid"_s).toString().toHtmlEscaped(),
                                                url.toHtmlEscaped()));
                    m_network->show();
                }
                setRunning(true);
            });
    // Whether the run prints what the project expects (sim.expect).
    m_expect->setWordWrap(true);
    m_expect->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_expect->hide();
    connect(m_core, &CoreConnection::simExpect, this,
            [this](const QStringList &matched, const QStringList &missing, const QString &needs, int seconds,
                   bool timedOut) {
                QStringList shown;
                for (const QString &p : missing)
                    shown << readablePattern(p);
                QString text;
                if (missing.isEmpty()) {
                    text = tr("✓ The output is what this project expects.");
                } else if (!timedOut) {
                    text = tr("Waiting for the expected output: %1 of %2 seen so far.")
                               .arg(matched.size())
                               .arg(matched.size() + missing.size());
                } else {
                    text = tr("Not seen yet (a board prints it within %1 s): %2").arg(seconds).arg(shown.join(u"; "_s));
                    if (needs == u"wifi")
                        text += u" "_s + tr("This project needs Wi-Fi, and the simulated network reaches nothing, "
                                             "so it may stop before that.");
                    else if (needs == u"parts")
                        text += u" "_s + tr("It needs its parts: add them in the Circuit dock, or what it prints "
                                             "about them may differ.");
                }
                m_expect->setText(text);
                m_expect->setToolTip(tr("What a board running this project prints, from esp32bench.dev: %1")
                                         .arg((matched + missing).join(u"; "_s)));
                m_expect->show();
            });
    connect(m_core, &CoreConnection::simWifiJoined, this, [this](const QString &ssid) {
        m_output->appendNotice(
            tr("Gave the sketch the simulated Wi-Fi settings (network %1, no password); it restarts to join.")
                .arg(ssid));
    });
    connect(m_core, &CoreConnection::simOutput, m_output, &ConsoleView::appendOutput);
    connect(m_core, &CoreConnection::simBacktrace, m_output, &ConsoleView::appendBacktrace);
    connect(m_core, &CoreConnection::simBootLoop, m_output, &ConsoleView::appendBootLoop);
    connect(m_core, &CoreConnection::simStopped, this, [this](const QString &reason) {
        m_output->appendNotice(tr("Simulator stopped: %1").arg(reason));
        m_status->setText(tr("Not running."));
        m_network->hide();
        setRunning(false);
    });
    connect(m_core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method.startsWith(u"sim."))
            m_output->appendNotice(tr("Simulator error: %1").arg(message));
    });
    setRunning(false);
}

QString SimulatorPanel::outputText() const
{
    return m_output->toPlainText();
}

QString SimulatorPanel::networkText() const
{
    return m_network->isHidden() ? QString() : m_network->text();
}

QString SimulatorPanel::clockText() const
{
    return m_clock->isHidden() ? QString() : m_clock->text();
}

QString SimulatorPanel::statusText() const
{
    return m_status->text();
}

void SimulatorPanel::send()
{
    if (!m_core->isSimRunning())
        return;
    const QString line = m_input->text() + m_ending->currentData().toString();
    if (line.isEmpty())
        return;
    m_core->writeSim(line.toUtf8());
    m_input->clear();
}

void SimulatorPanel::updatePause()
{
    const bool can = m_core->isSimRunning() && !m_core->isDebugging();
    const bool paused = m_core->isSimPaused();
    const QIcon icon = QIcon::fromTheme(paused ? u"media-playback-start"_s : u"media-playback-pause"_s);
    m_pauseAction->setEnabled(can);
    m_pauseAction->setText(paused ? tr("Resume simulation") : tr("Pause simulation"));
    m_pauseAction->setIcon(icon);
    m_pauseAction->setToolTip(paused ? tr("Run the firmware on from where it stopped (%1)")
                                           .arg(m_pauseAction->shortcut().toString(QKeySequence::NativeText))
                                     : tr("Stop the firmware where it is, to look at its pins and trace (%1)")
                                           .arg(m_pauseAction->shortcut().toString(QKeySequence::NativeText)));
    m_pause->setEnabled(can);
    m_pause->setIcon(icon);
    m_pause->setText(paused ? tr("Resume") : tr("Pause"));
    m_pause->setToolTip(m_pauseAction->toolTip());
    // A debug run is paused from the Debugger dock.
    m_pause->setVisible(!m_core->isDebugging());
}

void SimulatorPanel::setRunning(bool running)
{
    m_reset->setEnabled(running);
    m_stop->setEnabled(running);
    m_again->setVisible(!running && !m_lastProject.isEmpty());
    m_input->setEnabled(running);
    m_ending->setEnabled(running);
    m_send->setEnabled(running);
    updatePause();
}
