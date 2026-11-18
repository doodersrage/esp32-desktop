#include "SimulatorPanel.h"

#include "ConsoleView.h"
#include "CoreConnection.h"
#include "Labels.h"

#include <QComboBox>
#include <QHBoxLayout>
#include <QIcon>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

SimulatorPanel::SimulatorPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(tr("Not running. Build a project for an ESP32, ESP32-S3 or ESP32-C3 board, "
                             "then choose Run in simulator."),
                          this))
    , m_network(new QLabel(this))
    , m_reset(new QPushButton(QIcon::fromTheme(u"view-refresh"_s), tr("Reset"), this))
    , m_stop(new QPushButton(QIcon::fromTheme(u"media-playback-stop"_s), tr("Stop"), this))
    , m_again(new QPushButton(QIcon::fromTheme(u"media-playback-start"_s), tr("Run again"), this))
    , m_clear(new QPushButton(QIcon::fromTheme(u"edit-clear"_s), tr("Clear"), this))
    , m_output(new ConsoleView(this))
    , m_input(new QLineEdit(this))
    , m_ending(new QComboBox(this))
    , m_send(new QPushButton(tr("Send"), this))
{
    m_status->setWordWrap(true);
    m_status->setToolTip(tr("The simulator runs the firmware. Parts placed in the Circuit dock run "
                            "with it on the ESP32 and ESP32-C3; there is no Bluetooth."));
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
    connect(m_stop, &QPushButton::clicked, m_core, &CoreConnection::stopSim);
    m_again->setToolTip(tr("Run the same build again, without rebuilding"));
    connect(m_again, &QPushButton::clicked, this, [this] {
        if (!m_lastProject.isEmpty() && !m_core->isSimRunning())
            m_core->startSim(m_lastProject, m_lastBoard);
    });
    connect(m_clear, &QPushButton::clicked, m_output, &ConsoleView::clear);
    connect(m_send, &QPushButton::clicked, this, &SimulatorPanel::send);
    connect(m_input, &QLineEdit::returnPressed, this, &SimulatorPanel::send);

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_reset);
    top->addWidget(m_stop);
    top->addWidget(m_again);
    top->addWidget(m_clear);
    auto *bottom = new QHBoxLayout;
    bottom->addWidget(m_input, 1);
    bottom->addWidget(m_ending);
    bottom->addWidget(m_send);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_network);
    layout->addWidget(m_output, 1);
    layout->addLayout(bottom);

    connect(m_core, &CoreConnection::simStarted, this,
            [this](const QString &project, const QString &board, const QString &chip) {
                m_output->resetDecoder();
                m_output->appendNotice(tr("Started %1 for %2 on a simulated %3.")
                                           .arg(project, board, Labels::chipName(chip)));
                m_status->setText(tr("Running %1 on a simulated %2.").arg(project, Labels::chipName(chip)));
                m_lastProject = project;
                m_lastBoard = board;
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
    connect(m_core, &CoreConnection::simWifiJoined, this, [this](const QString &ssid) {
        m_output->appendNotice(
            tr("Gave the sketch the simulated Wi-Fi settings (network %1, no password); it restarts to join.")
                .arg(ssid));
    });
    connect(m_core, &CoreConnection::simOutput, m_output, &ConsoleView::appendBytes);
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

void SimulatorPanel::setRunning(bool running)
{
    m_reset->setEnabled(running);
    m_stop->setEnabled(running);
    m_again->setVisible(!running && !m_lastProject.isEmpty());
    m_input->setEnabled(running);
    m_ending->setEnabled(running);
    m_send->setEnabled(running);
}
