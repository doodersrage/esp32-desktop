// CircuitPanel and CoreConnection's circuit, model and control calls against a fake
// core.

#include "CircuitPanel.h"
#include "CoreConnection.h"

#include <QGraphicsView>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QMenu>
#include <QPointer>
#include <QTemporaryDir>
#include <QTest>
#include <QToolButton>

using namespace Qt::StringLiterals;

static QJsonObject model(const QString &id, const QString &name, const QString &look, const QJsonArray &pins,
                         const QJsonArray &controls, const QJsonObject &state)
{
    return {{u"schemaVersion"_s, u"1.0.0"_s}, {u"id"_s, id}, {u"name"_s, name}, {u"look"_s, look},
            {u"pins"_s, pins}, {u"controls"_s, controls}, {u"state"_s, state}, {u"script"_s, id + u".rhai"_s},
            {u"props"_s, QJsonObject{}}};
}

class FakeCore : public QObject {
    Q_OBJECT
public:
    QJsonObject saved{{u"schemaVersion"_s, u"1.1.0"_s},
                      {u"parts"_s, QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"kind"_s, u"led"_s}, {u"pin"_s, 2}}}}};
    QList<QJsonObject> controls;
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
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.16.0"_s}});
        } else if (m == u"circuit.models") {
            const QJsonObject off{{u"on"_s, false}};
            reply(id, QJsonObject{{u"models"_s, QJsonArray{
                model(u"led"_s, u"LED"_s, u"led"_s, {u"anode"_s}, {}, off),
                model(u"button"_s, u"Push button"_s, u"button"_s, {u"pin"_s},
                      {QJsonObject{{u"name"_s, u"press"_s}, {u"type"_s, u"momentary"_s}, {u"label"_s, u"Press"_s}}}, off),
                model(u"switch"_s, u"Slide switch"_s, u"switch"_s, {u"pin"_s},
                      {QJsonObject{{u"name"_s, u"toggle"_s}, {u"type"_s, u"toggle"_s}, {u"label"_s, u"On"_s}}}, off),
                model(u"rgb-led"_s, u"RGB LED"_s, u"rgb-led"_s, {u"red"_s, u"green"_s, u"blue"_s}, {},
                      QJsonObject{{u"red"_s, false}, {u"green"_s, false}, {u"blue"_s, false}}),
            }}});
        } else if (m == u"circuit.get") {
            reply(id, QJsonObject{{u"circuit"_s, saved}});
        } else if (m == u"circuit.set") {
            if (rejectNext) {
                rejectNext = false;
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32602}, {u"message"_s, u"two parts would drive GPIO 4"_s}}}});
                return;
            }
            saved = p.value(u"circuit"_s).toObject();
            reply(id, QJsonObject{});
        } else if (m == u"sim.start") {
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, false},
                                  {u"engine"_s, u"lcgamboa"_s}, {u"pins"_s, QJsonArray{2, 4}}, {u"partsActive"_s, true}});
        } else if (m == u"sim.control") {
            controls << p;
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

    static QJsonObject partsOf(const QJsonObject &circuit, int i)
    {
        return circuit.value(u"parts"_s).toArray().at(i).toObject();
    }

private slots:
    void editsAndRunsParts()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(700, 500);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        // The kinds of part come from the core, one menu entry each.
        QTRY_COMPARE(panel.modelIds(), (QStringList{u"led"_s, u"button"_s, u"switch"_s, u"rgb-led"_s}));
        QCOMPARE(panel.addPartButton()->menu()->actions().size(), 4);

        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s, u"GND"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QVERIFY(panel.item(u"led-1"_s));
        QVERIFY(panel.addPartButton()->isEnabled());

        // Adding saves through the core, with fresh ids.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        QJsonObject b = partsOf(fake.saved, 1);
        QCOMPARE(b.value(u"id"_s).toString(), u"button-1"_s);
        QCOMPARE(b.value(u"pins"_s).toObject().value(u"pin"_s).toInt(), 4);
        QCOMPARE(b.value(u"props"_s).toObject().value(u"activeLow"_s).toBool(), true);
        QCOMPARE(b.value(u"label"_s).toString(), u"Go"_s);
        QCOMPARE(fake.saved.value(u"schemaVersion"_s).toString(), u"1.1.0"_s);
        panel.addPart(u"rgb-led"_s, QJsonObject{{u"red"_s, 25}, {u"green"_s, 26}, {u"blue"_s, 27}}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 3);
        QCOMPARE(partsOf(fake.saved, 2).value(u"id"_s).toString(), u"rgb-led-1"_s);
        panel.addPart(u"switch"_s, QJsonObject{{u"pin"_s, 5}}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 4);

        // A refusal from the core is shown, and nothing changes.
        fake.rejectNext = true;
        panel.addPart(u"switch"_s, 4, false, {});
        QTRY_VERIFY(panel.statusText().contains(u"would drive"_s));
        QCOMPARE(panel.parts().size(), 4);

        // Running: parts show the state the core reports.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.simPartsActive());
        QTRY_VERIFY(panel.statusText().contains(u"Running"_s));
        fake.parts(QJsonArray{
            QJsonObject{{u"id"_s, u"led-1"_s}, {u"on"_s, true}, {u"state"_s, QJsonObject{{u"on"_s, true}}}},
            QJsonObject{{u"id"_s, u"rgb-led-1"_s},
                        {u"state"_s, QJsonObject{{u"red"_s, true}, {u"green"_s, false}, {u"blue"_s, true}}}}});
        QTRY_VERIFY(panel.isOn(u"led-1"_s));
        QCOMPARE(panel.partState(u"rgb-led-1"_s).value(u"blue"_s).toBool(), true);

        // Holding the mouse on a button presses it.
        const QPoint at = panel.view()->mapFromScene(panel.partCentre(u"button-1"_s));
        QTest::mousePress(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTRY_COMPARE(fake.controls.size(), 1);
        QCOMPARE(fake.controls[0].value(u"part"_s).toString(), u"button-1"_s);
        QCOMPARE(fake.controls[0].value(u"control"_s).toString(), u"press"_s);
        QCOMPARE(fake.controls[0].value(u"value"_s).toBool(), true);
        QTest::mouseRelease(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTRY_COMPARE(fake.controls.size(), 2);
        QCOMPARE(fake.controls[1].value(u"value"_s).toBool(), false);
        // Clicking a switch flips it: on, since it is off.
        const QPoint sw = panel.view()->mapFromScene(panel.partCentre(u"switch-1"_s));
        QTest::mouseClick(panel.view()->viewport(), Qt::LeftButton, {}, sw);
        QTRY_COMPARE(fake.controls.size(), 3);
        QCOMPARE(fake.controls[2].value(u"control"_s).toString(), u"toggle"_s);
        QCOMPARE(fake.controls[2].value(u"value"_s).toBool(), true);
        // A part's error is shown.
        fake.parts(QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"error"_s, u"the script failed"_s}}});
        QTRY_VERIFY(panel.statusText().contains(u"the script failed"_s));

        core.stopSim();
        QTRY_VERIFY(!panel.isOn(u"led-1"_s));
        QCOMPARE(panel.partState(u"rgb-led-1"_s).value(u"blue"_s).toBool(), false);
        // Not running: nothing is sent.
        QTest::mousePress(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTest::mouseRelease(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTest::qWait(100);
        QCOMPARE(fake.controls.size(), 3);

        panel.removePart(u"rgb-led-1"_s);
        QTRY_COMPARE(panel.parts().size(), 3);
    }
};

QTEST_MAIN(TestCircuit)
#include "tst_circuit.moc"
