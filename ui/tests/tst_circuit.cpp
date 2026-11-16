// CircuitPanel and CoreConnection's circuit and part calls against a fake core.

#include "CircuitPanel.h"
#include "CoreConnection.h"

#include <QGraphicsView>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QTemporaryDir>
#include <QTest>

using namespace Qt::StringLiterals;

class FakeCore : public QObject {
    Q_OBJECT
public:
    QJsonObject saved{{u"schemaVersion"_s, u"1.0.0"_s},
                      {u"parts"_s, QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"kind"_s, u"led"_s}, {u"pin"_s, 2}}}}};
    QList<QJsonObject> presses;
    bool rejectNext = false;

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

    void parts(const QJsonArray &changed)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.parts"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"parts"_s, changed}}}});
    }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.15.0"_s}});
        } else if (m == u"circuit.get") {
            reply(id, QJsonObject{{u"circuit"_s, saved}});
        } else if (m == u"circuit.set") {
            if (rejectNext) {
                rejectNext = false;
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32602}, {u"message"_s, u"two buttons on GPIO 4"_s}}}});
                return;
            }
            saved = p.value(u"circuit"_s).toObject();
            reply(id, QJsonObject{});
        } else if (m == u"sim.start") {
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, false},
                                  {u"engine"_s, u"lcgamboa"_s}, {u"pins"_s, QJsonArray{2, 4}}, {u"partsActive"_s, true}});
        } else if (m == u"sim.press") {
            presses << p;
            reply(id, QJsonObject{});
        } else {
            reply(id, QJsonObject{});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestCircuit : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void editsAndRunsParts()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(700, 400);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        // Selecting a board loads its saved circuit.
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s, u"GND"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QVERIFY(panel.item(u"led-1"_s));

        // Adding saves through the core, with a fresh id.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        const QJsonObject b = fake.saved.value(u"parts"_s).toArray().at(1).toObject();
        QCOMPARE(b.value(u"id"_s).toString(), u"button-1"_s);
        QCOMPARE(b.value(u"pin"_s).toInt(), 4);
        QCOMPARE(b.value(u"activeLow"_s).toBool(), true);
        QCOMPARE(b.value(u"label"_s).toString(), u"Go"_s);
        QVERIFY(panel.item(u"button-1"_s));
        panel.addPart(u"led"_s, 5, false, {});
        QTRY_COMPARE(panel.parts().size(), 3);
        QCOMPARE(fake.saved.value(u"parts"_s).toArray().at(2).toObject().value(u"id"_s).toString(), u"led-2"_s);

        // A refusal from the core is shown, and nothing changes.
        fake.rejectNext = true;
        panel.addPart(u"button"_s, 4, false, {});
        QTRY_VERIFY(panel.statusText().contains(u"two buttons"_s));
        QCOMPARE(panel.parts().size(), 3);

        panel.removePart(u"led-2"_s);
        QTRY_COMPARE(panel.parts().size(), 2);

        // Running: LEDs follow sim.parts, and holding the mouse on a button presses it.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.simPartsActive());
        QTRY_VERIFY(panel.statusText().contains(u"Running"_s));
        fake.parts(QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"on"_s, true}}});
        QTRY_VERIFY(panel.isOn(u"led-1"_s));
        const QPoint at = panel.view()->mapFromScene(panel.partCentre(u"button-1"_s));
        QTest::mousePress(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTRY_COMPARE(fake.presses.size(), 1);
        QCOMPARE(fake.presses[0].value(u"part"_s).toString(), u"button-1"_s);
        QCOMPARE(fake.presses[0].value(u"pressed"_s).toBool(), true);
        QTest::mouseRelease(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTRY_COMPARE(fake.presses.size(), 2);
        QCOMPARE(fake.presses[1].value(u"pressed"_s).toBool(), false);

        core.stopSim();
        QTRY_VERIFY(!panel.isOn(u"led-1"_s));
        // Not running: pressing does nothing.
        QTest::mousePress(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTest::mouseRelease(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTest::qWait(100);
        QCOMPARE(fake.presses.size(), 2);
    }
};

QTEST_MAIN(TestCircuit)
#include "tst_circuit.moc"
