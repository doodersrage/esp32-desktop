#include "SerialMonitor.h"

#include "ConsoleView.h"
#include "CoreConnection.h"

#include <QComboBox>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QIcon>
#include <QJsonArray>
#include <QJsonObject>
#include <QLineEdit>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QScrollBar>
#include <QTextCursor>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
// Must match the rates the core accepts (schema/rpc/serial.open.json).
constexpr int BaudRates[] = {9600, 19200, 38400, 57600, 74880, 115200, 230400, 460800, 921600, 2000000};
constexpr int DefaultBaud = 115200;
}

SerialMonitor::SerialMonitor(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_port(new QComboBox(this))
    , m_baud(new QComboBox(this))
    , m_open(new QPushButton(this))
    , m_reset(new QPushButton(QIcon::fromTheme(u"view-refresh"_s), tr("Reset board"), this))
    , m_clear(new QPushButton(QIcon::fromTheme(u"edit-clear"_s), tr("Clear"), this))
    , m_output(new ConsoleView(this))
    , m_input(new QLineEdit(this))
    , m_ending(new QComboBox(this))
    , m_send(new QPushButton(tr("Send"), this))
{
    m_port->setMinimumContentsLength(14);
    m_port->setToolTip(tr("Serial port"));
    for (int b : BaudRates)
        m_baud->addItem(tr("%1 baud").arg(b), b);
    m_baud->setCurrentIndex(m_baud->findData(DefaultBaud));

    m_reset->setToolTip(tr("Pulse the reset line, as esptool does. Boards without auto-reset ignore it."));
    m_clear->setToolTip(tr("Clear the output"));


    m_input->setPlaceholderText(tr("Text to send to the board"));
    m_ending->addItem(tr("LF"), u"\n"_s);
    m_ending->addItem(tr("CR LF"), u"\r\n"_s);
    m_ending->addItem(tr("No line ending"), QString());
    m_ending->setToolTip(tr("What to add after each line sent"));

    connect(m_open, &QPushButton::clicked, this, [this] {
        if (m_open_) {
            m_core->closeSerial();
            return;
        }
        if (m_port->currentText().isEmpty())
            return;
        m_open->setEnabled(false);
        m_core->openSerial(m_port->currentData().toString(), m_baud->currentData().toInt());
    });
    connect(m_reset, &QPushButton::clicked, m_core, &CoreConnection::resetSerial);
    connect(m_clear, &QPushButton::clicked, m_output, &QPlainTextEdit::clear);
    connect(m_send, &QPushButton::clicked, this, &SerialMonitor::send);
    new InputHistory(m_input); // Up and Down recall what was sent
    connect(m_input, &QLineEdit::returnPressed, this, &SerialMonitor::send);

    auto *top = new QHBoxLayout;
    top->addWidget(m_port, 1);
    top->addWidget(m_baud);
    top->addWidget(m_open);
    top->addWidget(m_reset);
    top->addWidget(m_clear);
    auto *bottom = new QHBoxLayout;
    bottom->addWidget(m_input, 1);
    bottom->addWidget(m_ending);
    bottom->addWidget(m_send);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_output, 1);
    m_tools = new ConsoleBar(m_output, m_core, u"serial/timestamps"_s, this);
    layout->addWidget(m_tools);
    layout->addLayout(bottom);

    connect(m_core, &CoreConnection::devicesListed, this, &SerialMonitor::setPorts);
    connect(m_core, &CoreConnection::serialOpened, this, [this](const QString &path, int baud) {
        m_output->resetDecoder();
        m_output->appendNotice(tr("Connected to %1 at %2 baud.").arg(path).arg(baud));
        // Linux raises DTR and RTS while opening a port, and the built-in USB of the
        // S3, C3 and C6 resets the chip on that; say so, or the boot log looks like a crash.
        const int i = m_port->findData(path);
        if (i >= 0 && m_port->itemData(i, BridgeRole).toString() == u"usb-serial-jtag")
            m_output->appendNotice(tr("Boards with built-in USB restart when their port is opened."));
        setOpen(true);
    });
    connect(m_core, &CoreConnection::serialData, m_output, &ConsoleView::appendBytes);
    connect(m_core, &CoreConnection::serialClosed, this, [this](const QString &reason) {
        if (m_open_)
            m_output->appendNotice(tr("Disconnected: %1").arg(reason));
        else if (!reason.isEmpty())
            m_output->appendNotice(tr("Could not open the port: %1").arg(reason));
        setOpen(false);
    });
    connect(m_core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method.startsWith(u"serial."))
            m_output->appendNotice(tr("Serial error: %1").arg(message));
    });
    connect(m_core, &CoreConnection::stateChanged, this, [this] { setOpen(m_core->isSerialOpen()); });
    // A project's baud rate is the right default for its board.
    connect(m_core, &CoreConnection::projectLoaded, this, [this](const QJsonObject &result) {
        const QJsonObject p = result.value(u"project"_s).toObject();
        const int baud = p.value(u"baud"_s).toInt();
        if (baud > 0)
            m_projectBauds.insert(p.value(u"id"_s).toString(), baud);
        suggestBaud(baud);
    });
    setOpen(false);
}

QString SerialMonitor::outputText() const
{
    return m_output->toPlainText();
}

void SerialMonitor::openPort(const QString &path)
{
    if (m_open_) {
        if (m_port->currentData().toString() == path)
            return;
        m_core->closeSerial();
    }
    int i = m_port->findData(path);
    if (i < 0) {
        m_port->addItem(path, path);
        i = m_port->count() - 1;
    }
    m_port->setCurrentIndex(i);
    m_open->setEnabled(false);
    m_core->openSerial(path, m_baud->currentData().toInt());
}

void SerialMonitor::setPorts(const QJsonArray &ports)
{
    const QString current = m_port->currentData().toString();
    const QSignalBlocker block(m_port);
    m_port->clear();
    for (const QJsonValue &v : ports) {
        const QJsonObject p = v.toObject();
        const QString path = p.value(u"path"_s).toString();
        m_port->addItem(path, path);
        m_port->setItemData(m_port->count() - 1, p.value(u"bridge"_s).toString(), BridgeRole);
    }
    // Keep the open (or last chosen) port selected even if it just went away.
    if (!current.isEmpty()) {
        int i = m_port->findData(current);
        if (i < 0 && m_open_) {
            m_port->addItem(current, current);
            i = m_port->count() - 1;
        }
        if (i >= 0)
            m_port->setCurrentIndex(i);
    }
    setOpen(m_open_);
}

void SerialMonitor::suggestBaud(int baud)
{
    if (m_open_ || baud <= 0)
        return;
    const int i = m_baud->findData(baud);
    if (i >= 0)
        m_baud->setCurrentIndex(i);
}

void SerialMonitor::send()
{
    if (!m_open_)
        return;
    const QString line = m_input->text() + m_ending->currentData().toString();
    if (line.isEmpty())
        return;
    m_core->writeSerial(line.toUtf8());
    m_input->clear();
}

void SerialMonitor::setOpen(bool open)
{
    m_open_ = open;
    const bool connected = m_core->state() == CoreConnection::State::Connected;
    m_open->setText(open ? tr("Close") : tr("Open"));
    m_open->setIcon(QIcon::fromTheme(open ? u"network-disconnect"_s : u"network-connect"_s));
    m_open->setEnabled(connected && (open || m_port->count() > 0));
    m_port->setEnabled(!open);
    m_baud->setEnabled(!open);
    m_reset->setEnabled(open);
    m_input->setEnabled(open);
    m_send->setEnabled(open);
    m_ending->setEnabled(open);
}
