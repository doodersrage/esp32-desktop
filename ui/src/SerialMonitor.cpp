#include "SerialMonitor.h"

#include <QFileInfo>
#include <QDir>
#include <QDateTime>
#include <QFileDialog>
#include <QStandardPaths>

#include "ConsoleView.h"
#include "CoreConnection.h"
#include "Preferences.h"
#include "SimulatorPanel.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QIcon>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QMenu>
#include <QSettings>
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
    , m_sendFile(new QPushButton(QIcon::fromTheme(u"document-send"_s), tr("Send file…"), this))
    , m_hex(new QCheckBox(tr("Hex"), this))
    , m_expect(new QLabel(this))
{
    // Whether the board prints what the project flashed to it expects (serial.expect).
    m_expect->setWordWrap(true);
    m_expect->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_expect->hide();
    connect(m_core, &CoreConnection::serialExpect, this,
            [this](const QStringList &matched, const QStringList &missing, const QString &, int seconds, bool timedOut) {
                QStringList shown;
                for (const QString &p : missing)
                    shown << SimulatorPanel::readablePattern(p);
                QString text;
                if (missing.isEmpty())
                    text = tr("✓ The board prints what this project expects.");
                else if (!timedOut)
                    text = tr("Waiting for the expected output: %1 of %2 seen so far.")
                               .arg(matched.size())
                               .arg(matched.size() + missing.size());
                else
                    text = tr("Not seen yet (a board prints it within %1 s): %2. If the board started before the port "
                              "opened, press Reset board to see it start again.")
                               .arg(seconds)
                               .arg(shown.join(u"; "_s));
                m_expect->setText(text);
                m_expect->setToolTip(tr("What a board running this project prints, from esp32bench.dev: %1")
                                         .arg((matched + missing).join(u"; "_s)));
                m_expect->show();
            });
    m_hex->setToolTip(tr("Show what the board sends as hex bytes, and send what you type as hex (\"01 a0 ff\")"));
    connect(m_hex, &QCheckBox::toggled, this, [this](bool on) {
        m_output->setHex(on);
        m_ending->setEnabled(!on);
        m_input->setPlaceholderText(on ? tr("Hex bytes to send, such as 01 a0 ff") : tr("Text to send to the board"));
    });
    m_port->setMinimumContentsLength(14);
    m_port->setAccessibleName(tr("Serial port"));
    m_baud->setAccessibleName(tr("Baud rate"));
    m_output->setAccessibleName(tr("What the board sent"));
    m_input->setAccessibleName(tr("Text to send"));
    m_ending->setAccessibleName(tr("Line ending"));
    m_port->setToolTip(tr("Serial port"));
    for (int b : BaudRates)
        m_baud->addItem(tr("%1 baud").arg(b), b);
    // The speed used last, else the usual one.
    const int lastBaud = m_baud->findData(QSettings().value(u"serial/lastBaud"_s, DefaultBaud).toInt());
    m_baud->setCurrentIndex(lastBaud >= 0 ? lastBaud : m_baud->findData(DefaultBaud));

    m_reset->setToolTip(tr("Pulse the reset line, as esptool does. Boards without auto-reset ignore it."));
    m_clear->setToolTip(tr("Clear the output"));


    m_input->setPlaceholderText(tr("Text to send to the board"));
    m_ending->addItem(tr("LF"), u"\n"_s);
    m_ending->addItem(tr("CR LF"), u"\r\n"_s);
    m_ending->addItem(tr("No line ending"), QString());
    m_ending->setToolTip(tr("What to add after each line sent"));

    connect(m_open, &QPushButton::clicked, this, [this] {
        if (m_open_) {
            QSettings().setValue(u"serial/open"_s, false); // closed on purpose: not reopened at start
            m_core->closeSerial();
            return;
        }
        if (m_port->currentText().isEmpty())
            return;
        m_open->setEnabled(false);
        m_expect->hide();
        m_core->openSerial(m_port->currentData().toString(), m_baud->currentData().toInt());
    });
    connect(m_reset, &QPushButton::clicked, m_core, &CoreConnection::resetSerial);
    connect(m_clear, &QPushButton::clicked, m_output, &QPlainTextEdit::clear);
    m_record = new QPushButton(QIcon::fromTheme(u"media-record"_s), tr("Record…"), this);
    m_record->setToolTip(tr("Write everything the board sends from now on to a file, however long it runs"));
    m_record->setCheckable(true);
    connect(m_record, &QPushButton::clicked, this, [this] {
        m_record->setChecked(!m_recordingTo.isEmpty()); // the core's answer sets it
        if (!m_recordingTo.isEmpty()) {
            recordTo({});
            return;
        }
        const QString dir = QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation);
        const QString name = u"esp32bench-serial-%1.log"_s.arg(QDateTime::currentDateTime().toString(u"yyyyMMdd-HHmmss"_s));
        const QString path = QFileDialog::getSaveFileName(this, tr("Record the board's output"), QDir(dir).filePath(name),
                                                          tr("Log files (*.log *.txt);;All files (*)"), nullptr,
                                                          QFileDialog::DontConfirmOverwrite);
        if (!path.isEmpty())
            recordTo(path);
    });
    connect(m_core, &CoreConnection::serialRecording, this, [this](bool on, const QString &path, qint64 bytes) {
        m_recordingTo = on ? path : QString();
        m_record->setChecked(on);
        m_record->setText(on ? tr("Stop recording") : tr("Record…"));
        m_output->appendNotice(on ? tr("Recording to %1 (added to the end of the file).").arg(path)
                                  : tr("Recorded %1 bytes to %2.").arg(bytes).arg(path));
    });
    connect(m_core, &CoreConnection::serialRecordFailed, this, [this](const QString &message, qint64 bytes) {
        m_output->appendNotice(tr("Recording stopped: %1. %2 bytes were written to %3.")
                                   .arg(message)
                                   .arg(bytes)
                                   .arg(m_recordingTo));
        m_recordingTo.clear();
        m_record->setChecked(false);
        m_record->setText(tr("Record…"));
    });
    connect(m_send, &QPushButton::clicked, this, &SerialMonitor::send);
    m_sendFile->setToolTip(tr("Send a text file to the board line by line, such as a list of commands"));
    connect(m_sendFile, &QPushButton::clicked, this, [this] {
        QString path;
        int delay = 0;
        if (m_core->isSerialOpen() && ConsoleBar::askSendFile(this, u"serial/sendFile"_s, &path, &delay)) {
            m_output->appendNotice(tr("Sending %1…").arg(QFileInfo(path).fileName()));
            m_core->sendSerialFile(path, m_ending->currentData().toString(), delay);
        }
    });
    connect(m_core, &CoreConnection::fileSent, this, [this](const QString &source, int) {
        if (source == u"serial")
            m_output->appendNotice(tr("Finished sending the file."));
    });
    new InputHistory(m_input); // Up and Down recall what was sent
    connect(m_input, &QLineEdit::returnPressed, this, &SerialMonitor::send);

    auto *top = new QHBoxLayout;
    top->addWidget(m_port, 1);
    top->addWidget(m_baud);
    top->addWidget(m_open);
    top->addWidget(m_reset);
    top->addWidget(m_record);
    top->addWidget(m_clear);
    // Unreadable output: most likely the wrong speed; one click tries the usual one.
    m_garbled = new QLabel(this);
    m_garbled->setWordWrap(true);
    m_garbledSwitch = new QPushButton(tr("Switch speed"), this);
    m_garbledSwitch->hide();
    m_garbled->hide();
    connect(m_core, &CoreConnection::serialGarbled, this, [this](int baud, int suggest) {
        m_garbled->setText(suggest > 0 ? tr("This looks like the wrong speed (%1 baud).").arg(baud)
                                       : tr("This looks unreadable at %1 baud. Try the speed the firmware uses "
                                            "(Serial.begin in its code).")
                                             .arg(baud));
        m_garbled->show();
        m_garbledSwitch->setVisible(suggest > 0);
        m_garbledSwitch->setText(tr("Switch to %1").arg(suggest));
        m_garbledSwitch->setProperty("baud", suggest);
    });
    connect(m_garbledSwitch, &QPushButton::clicked, this, [this] {
        const int i = m_baud->findData(m_garbledSwitch->property("baud").toInt());
        const QString path = m_port->currentData().toString();
        if (i < 0 || path.isEmpty())
            return;
        m_garbled->hide();
        m_garbledSwitch->hide();
        m_core->closeSerial();
        m_baud->setCurrentIndex(i);
        openPort(path);
    });
    // Quick-send: lines kept for the project shown (in the core), one button each.
    m_quick = new QHBoxLayout;
    m_keep = new QPushButton(QIcon::fromTheme(u"list-add"_s), tr("Keep"), this);
    m_keep->setToolTip(tr("Keep the line typed as a button that sends it (for this project)"));
    connect(m_keep, &QPushButton::clicked, this, &SerialMonitor::keepInput);
    connect(m_core, &CoreConnection::commandsListed, this, [this](const QString &project, const QStringList &lines) {
        if (project == m_project) {
            m_commands = lines;
            showCommands();
        }
    });
    connect(m_core, &CoreConnection::stateChanged, this, [this](CoreConnection::State st) {
        if (st == CoreConnection::State::Connected)
            m_core->getCommands(m_project);
    });
    auto *bottom = new QHBoxLayout;
    bottom->addWidget(m_input, 1);
    bottom->addWidget(m_ending);
    bottom->addWidget(m_hex);
    bottom->addWidget(m_keep);
    bottom->addWidget(m_send);
    bottom->addWidget(m_sendFile);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    auto *garbled = new QHBoxLayout;
    garbled->addWidget(m_garbled, 1);
    garbled->addWidget(m_garbledSwitch);
    layout->addLayout(garbled);
    layout->addWidget(m_expect);
    layout->addWidget(m_output, 1);
    m_tools = new ConsoleBar(m_output, m_core, u"serial/timestamps"_s, this);
    layout->addWidget(m_tools);
    layout->addLayout(m_quick);
    layout->addLayout(bottom);

    connect(m_core, &CoreConnection::devicesListed, this, &SerialMonitor::setPorts);
    connect(m_core, &CoreConnection::serialOpened, this, [this](const QString &path, int baud) {
        m_output->resetDecoder();
        m_output->appendNotice(tr("Connected to %1 at %2 baud.").arg(path).arg(baud));
        m_tools->setContext(tr("a board on %1 at %2 baud").arg(path).arg(baud));
        QSettings settings;
        settings.setValue(u"serial/lastPort"_s, path);
        settings.setValue(u"serial/lastBaud"_s, baud);
        settings.setValue(u"serial/open"_s, true);
        // Linux raises DTR and RTS while opening a port, and the built-in USB of the
        // S3, C3 and C6 resets the chip on that; say so, or the boot log looks like a crash.
        const int i = m_port->findData(path);
        if (i >= 0 && m_port->itemData(i, BridgeRole).toString() == u"usb-serial-jtag")
            m_output->appendNotice(tr("Boards with built-in USB restart when their port is opened."));
        setOpen(true);
    });
    connect(m_core, &CoreConnection::serialData, m_output, &ConsoleView::appendOutput);
    connect(m_core, &CoreConnection::serialBacktrace, m_output, &ConsoleView::appendBacktrace);
    connect(m_core, &CoreConnection::serialBootLoop, m_output, &ConsoleView::appendBootLoop);
    // The core reopens a port that comes back (a reset over the board's own USB, or
    // plugging it in again).
    connect(m_core, &CoreConnection::serialLost, this, [this] {
        m_output->appendNotice(tr("The board went away; waiting for it to come back…"));
        m_input->setEnabled(false);
        m_send->setEnabled(false);
    });
    connect(m_core, &CoreConnection::serialReconnected, this, [this] {
        m_output->resetDecoder();
        m_output->appendNotice(tr("Reconnected."));
        setOpen(true);
    });
    connect(m_core, &CoreConnection::serialClosed, this, [this](const QString &reason) {
        if (!m_recordingTo.isEmpty()) {
            m_output->appendNotice(tr("Stopped recording to %1.").arg(m_recordingTo));
            m_recordingTo.clear();
            m_record->setChecked(false);
            m_record->setText(tr("Record…"));
        }
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

QString SerialMonitor::expectText() const
{
    return m_expect->isHidden() ? QString() : m_expect->text();
}

void SerialMonitor::openPort(const QString &path, const QString &expect, const QString &board)
{
    m_expect->hide();
    m_garbled->hide();
    m_garbledSwitch->hide();
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
    m_core->openSerial(path, m_baud->currentData().toInt(), expect, board);
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
    // At first, the port used last time; reopened if it was open when the app quit.
    if (m_restore && !m_restored && !ports.isEmpty()) {
        m_restored = true;
        QSettings settings;
        const QString last = settings.value(u"serial/lastPort"_s).toString();
        const int i = m_port->findData(last);
        if (i >= 0 && current.isEmpty()) {
            m_port->setCurrentIndex(i);
            if (settings.value(u"serial/open"_s).toBool() && Preferences::reopenSerial() && !m_open_
                && m_core->state() == CoreConnection::State::Connected) {
                setOpen(false);
                QMetaObject::invokeMethod(this, [this, last] { openPort(last); }, Qt::QueuedConnection);
            }
            setOpen(m_open_);
            return;
        }
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
    if (m_hex->isChecked()) {
        const QByteArray bytes = hexBytes(m_input->text());
        if (bytes.isEmpty()) {
            m_output->appendNotice(tr("Not hex bytes: type pairs of digits 0-9 and a-f, such as 01 a0 ff."));
            return;
        }
        m_core->writeSerial(bytes);
        m_input->clear();
        return;
    }
    const QString line = m_input->text() + m_ending->currentData().toString();
    if (line.isEmpty())
        return;
    m_core->writeSerial(line.toUtf8());
    m_input->clear();
}

QByteArray SerialMonitor::hexBytes(const QString &text)
{
    QString digits = text;
    digits.remove(u' ').remove(u',').remove(u'\t');
    if (digits.startsWith(u"0x"_s, Qt::CaseInsensitive) && !digits.contains(u' '))
        digits = digits.mid(2);
    if (digits.isEmpty() || digits.size() % 2 != 0)
        return {};
    for (const QChar c : std::as_const(digits)) {
        if (!c.isDigit() && !(c.toLower() >= u'a' && c.toLower() <= u'f'))
            return {};
    }
    return QByteArray::fromHex(digits.toLatin1());
}

void SerialMonitor::recordTo(const QString &path)
{
    m_core->recordSerial(path);
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
    m_record->setEnabled(open);
    m_input->setEnabled(open);
    for (int i = 0; m_quick && i < m_quick->count(); ++i) {
        if (QWidget *w = m_quick->itemAt(i)->widget())
            w->setEnabled(open);
    }
    m_send->setEnabled(open);
    m_sendFile->setEnabled(open);
    m_ending->setEnabled(open);
}

void SerialMonitor::setProject(const QString &project)
{
    if (project == m_project)
        return;
    m_project = project;
    m_commands.clear();
    showCommands();
    if (m_core->state() == CoreConnection::State::Connected)
        m_core->getCommands(project);
}

void SerialMonitor::keepInput()
{
    const QString line = m_input->text();
    if (line.trimmed().isEmpty() || m_commands.contains(line))
        return;
    m_core->setCommands(m_project, m_commands + QStringList{line});
}

void SerialMonitor::forgetCommand(const QString &line)
{
    QStringList kept = m_commands;
    kept.removeAll(line);
    m_core->setCommands(m_project, kept);
}

void SerialMonitor::sendCommand(const QString &line)
{
    if (!m_open_)
        return;
    if (m_hex->isChecked()) {
        const QByteArray bytes = hexBytes(line);
        if (!bytes.isEmpty())
            m_core->writeSerial(bytes);
        return;
    }
    m_core->writeSerial((line + m_ending->currentData().toString()).toUtf8());
}

void SerialMonitor::showCommands()
{
    while (QLayoutItem *item = m_quick->takeAt(0)) {
        delete item->widget();
        delete item;
    }
    for (const QString &line : std::as_const(m_commands)) {
        auto *b = new QPushButton(line, this);
        b->setToolTip(tr("Send “%1”; right-click to forget it").arg(line));
        b->setEnabled(m_open_);
        b->setContextMenuPolicy(Qt::CustomContextMenu);
        connect(b, &QPushButton::clicked, this, [this, line] { sendCommand(line); });
        connect(b, &QPushButton::customContextMenuRequested, this, [this, b, line](const QPoint &pos) {
            QMenu menu(this);
            menu.addAction(QIcon::fromTheme(u"list-remove"_s), tr("Forget this button"), this,
                           [this, line] { forgetCommand(line); });
            menu.exec(b->mapToGlobal(pos));
        });
        m_quick->addWidget(b);
    }
    if (!m_commands.isEmpty())
        m_quick->addStretch(1);
}
