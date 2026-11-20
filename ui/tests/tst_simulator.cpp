// SimulatorPanel and CoreConnection's sim calls against a fake core.

#include "CoreConnection.h"
#include "SimulatorPanel.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QLineEdit>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QCheckBox>
#include <QSettings>
#include <QStandardPaths>
#include <QTest>

using namespace Qt::StringLiterals;

class FakeCore : public QObject {
    Q_OBJECT
public:
    QStringList calls;
    QList<QByteArray> written;
    bool lastIcount = false; // the last sim.start's icount

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

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void output(const QByteArray &b)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.output"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"data"_s, QString::fromLatin1(b.toBase64())},
                                        {u"text"_s, QString::fromUtf8(b)}}}});
    }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.24.0"_s}});
            return;
        }
        calls << m;
        if (m == u"sim.start") {
            if (p.value(u"board"_s).toString() == u"esp32c6") {
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32005},
                                               {u"message"_s, u"esp32c6 is workbench-only"_s}}}});
                return;
            }
            output("ets Jul 29 2019\r\n"); // before the reply
            QJsonObject r{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}};
            if (p.value(u"project"_s).toString() == u"web-led"_s) {
                r[u"network"_s] = QJsonObject{{u"ssid"_s, u"PICSimLabWifi"_s},
                                              {u"webUrl"_s, u"http://127.0.0.1:41823/"_s}};
                reply(id, r);
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.wifiJoined"_s},
                      {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"ssid"_s, u"PICSimLabWifi"_s}}}});
                return;
            }
            reply(id, r);
            output("esp32bench-hello ready\n");
            lastIcount = p.value(u"icount"_s).toBool();
        } else if (m == u"sim.write") {
            written << QByteArray::fromBase64(p.value(u"data"_s).toString().toLatin1());
            reply(id, QJsonObject{});
        } else if (m == u"sim.reset") {
            reply(id, QJsonObject{});
            output("rst:0x1\n");
        } else {
            reply(id, QJsonObject{});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestSimulator : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void initTestCase()
    {
        // An organisation, so QSettings can save (on Windows, a registry key).
        QCoreApplication::setOrganizationName(u"esp32bench-tests"_s);
        QStandardPaths::setTestModeEnabled(true);
    }
    void runsWithConsoleInputResetAndStop()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SimulatorPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QTRY_VERIFY(panel.outputText().contains(u"esp32bench-hello ready"_s));
        // Output that arrived before the reply comes first, without CRs.
        QVERIFY(panel.outputText().indexOf(u"ets Jul 29 2019\n"_s)
                < panel.outputText().indexOf(u"esp32bench-hello ready"_s));
        QVERIFY(panel.statusText().contains(u"simulated ESP32"_s));

        panel.input()->setText(u"ping"_s);
        QTest::keyClick(panel.input(), Qt::Key_Return);
        QTRY_COMPARE(fake.written, QList<QByteArray>{QByteArray("ping\n")});

        panel.resetButton()->click();
        QTRY_VERIFY(panel.outputText().contains(u"rst:0x1"_s));

        panel.stopButton()->click();
        QTRY_VERIFY(!core.isSimRunning());
        QTRY_VERIFY(fake.calls.contains(u"core.unsubscribe"_s));
        QVERIFY(!panel.input()->isEnabled());

        // Stopped: Run again starts the same build, with no rebuild.
        QVERIFY(!panel.runAgainButton()->isHidden());
        QCOMPARE(fake.calls.count(u"sim.start"_s), 1);
        panel.runAgainButton()->click();
        QTRY_VERIFY(core.isSimRunning());
        QCOMPARE(fake.calls.count(u"sim.start"_s), 2);
        QVERIFY(!fake.calls.contains(u"build.start"_s));
        QVERIFY(panel.runAgainButton()->isHidden());
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());
    }

    void showsTheSimulatedNetwork()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SimulatorPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(panel.networkText().isEmpty());
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());

        core.startSim(u"web-led"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(panel.networkText().contains(u"PICSimLabWifi"_s));
        QVERIFY(panel.networkText().contains(u"href=\"http://127.0.0.1:41823/\""_s));
        QTRY_VERIFY(panel.outputText().contains(u"simulated Wi-Fi settings"_s));
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());
        QVERIFY(panel.networkText().isEmpty());
    }

    void sendsChipLikeTimingWhenChosen()
    {
        QSettings().remove(u"simulator/icount"_s);
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SimulatorPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        QVERIFY(!panel.icountBox()->isChecked());
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(!fake.lastIcount);
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());

        panel.icountBox()->setChecked(true);
        QVERIFY(QSettings().value(u"simulator/icount"_s).toBool());
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(fake.lastIcount);
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());
        QSettings().remove(u"simulator/icount"_s);
    }

    void showsWhyABoardCannotBeSimulated()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SimulatorPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"blink"_s, u"esp32c6"_s);
        QTRY_VERIFY(panel.outputText().contains(u"workbench-only"_s));
        QVERIFY(!core.isSimRunning());
        // Nothing ran, so there is nothing to run again.
        QVERIFY(panel.runAgainButton()->isHidden());
    }
};

QTEST_MAIN(TestSimulator)
#include "tst_simulator.moc"
