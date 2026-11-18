// SerialMonitor and CoreConnection's serial calls against a fake core.

#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "MainWindow.h"
#include "SerialMonitor.h"

#include <QComboBox>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLineEdit>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QStandardPaths>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

// Knows core.hello, device.watch, serial.open (sends output *before* its reply),
// serial.write (records the bytes) and core.unsubscribe. pushClosed() unplugs.
class FakeCore : public QObject {
    Q_OBJECT
public:
    QList<QByteArray> written;
    QStringList opened;

    explicit FakeCore(const QString &path)
        : m_server(new QLocalServer(this))
    {
        QLocalServer::removeServer(path);
        QVERIFY(m_server->listen(path));
        connect(m_server, &QLocalServer::newConnection, this, [this] {
            m_sock = m_server->nextPendingConnection();
            connect(m_sock, &QLocalSocket::readyRead, m_sock, [this] {
                while (m_sock->canReadLine())
                    handle(QJsonDocument::fromJson(m_sock->readLine()).object());
            });
        });
    }

    void pushData(const QByteArray &bytes)
    {
        send(event(u"serial.data"_s, {{u"data"_s, QString::fromLatin1(bytes.toBase64())},
                                      {u"text"_s, QString::fromUtf8(bytes)}}));
    }
    void pushClosed() { send(event(u"serial.closed"_s, {{u"reason"_s, u"the port was closed"_s}})); }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    static QJsonObject event(const QString &method, QJsonObject params)
    {
        params[u"subscription"_s] = 4;
        return {{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, method}, {u"params"_s, params}};
    }
    void reply(const QJsonValue &id, const QJsonValue &result)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, result}});
    }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s},
                                  {u"protocol"_s, u"0.20.0"_s}});
        } else if (m == u"device.watch") {
            reply(id, QJsonObject{{u"subscription"_s, 1},
                                  {u"ports"_s, QJsonArray{QJsonObject{{u"path"_s, u"/dev/ttyUSB0"_s},
                                                                      {u"bridge"_s, u"usb-serial-jtag"_s}}}}});
        } else if (m == u"serial.open") {
            opened << p.value(u"path"_s).toString() + u"@"_s + QString::number(p.value(u"baud"_s).toInt());
            // Output that beats the reply; the UI must still show it, in order.
            // "é" is split across two chunks to check decoding keeps state.
            pushData("ets Jul 29 2019\r\ncaf\xc3");
            pushData("\xa9 ok\n");
            reply(id, QJsonObject{{u"subscription"_s, 4}});
        } else if (m == u"serial.write") {
            written << QByteArray::fromBase64(p.value(u"data"_s).toString().toLatin1());
            reply(id, QJsonObject{});
        } else if (m == u"core.unsubscribe") {
            reply(id, QJsonObject{});
        }
    }

    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestSerialMonitor : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    // Keep QSettings away from the real configuration.
    void initTestCase() { QStandardPaths::setTestModeEnabled(true); }

    void opensShowsOutputAndSends()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        SerialMonitor *mon = window.serialMonitor();
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        // The Devices panel subscribes; the monitor's port list follows it.
        QTRY_COMPARE(mon->portPicker()->count(), 1);

        mon->openButton()->click();
        QTRY_VERIFY(core.isSerialOpen());
        QCOMPARE(fake.opened, QStringList{u"/dev/ttyUSB0@115200"_s});
        QTRY_VERIFY(mon->outputText().contains(u"café ok"_s));
        QVERIFY(mon->outputText().contains(u"ets Jul 29 2019\ncafé"_s)); // \r removed
        QVERIFY(mon->outputText().contains(u"restart when their port is opened"_s));

        mon->input()->setText(u"SET wifi_ssid home"_s);
        QTest::keyClick(mon->input(), Qt::Key_Return);
        QTRY_COMPARE(fake.written.size(), 1);
        QCOMPARE(fake.written.at(0), QByteArray("SET wifi_ssid home\n"));
        QVERIFY(mon->input()->text().isEmpty());

        fake.pushClosed();
        QTRY_VERIFY(!core.isSerialOpen());
        QVERIFY(mon->outputText().contains(u"Disconnected"_s));
        QVERIFY(!mon->input()->isEnabled());
    }

    void doubleClickingAPortOpensIt()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        core.start();
        QTRY_COMPARE(window.devicesPanel()->tree()->topLevelItemCount(), 1);
        QTreeWidgetItem *item = window.devicesPanel()->tree()->topLevelItem(0);
        emit window.devicesPanel()->tree()->itemActivated(item, 0);
        QTRY_VERIFY(core.isSerialOpen());
        QCOMPARE(fake.opened, QStringList{u"/dev/ttyUSB0@115200"_s});
    }
};

QTEST_MAIN(TestSerialMonitor)
#include "tst_serialmonitor.moc"
