// TracePanel and CoreConnection's trace calls against a fake core.

#include "CoreConnection.h"
#include "TracePanel.h"

#include <QComboBox>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QSignalSpy>
#include <cmath>
#include <QTest>

using namespace Qt::StringLiterals;

static QJsonObject edge(qint64 t, int gpio, bool level, bool input = false)
{
    QJsonObject o{{u"t"_s, t}, {u"gpio"_s, gpio}, {u"level"_s, level}};
    if (input)
        o[u"input"_s] = true;
    return o;
}

class FakeCore : public QObject {
    Q_OBJECT
public:
    QJsonArray pinsToReport{2, 4};
    QList<QJsonObject> saves; // sim.traceSave params
    QString saveError;

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

    void trace(const QJsonArray &edges)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.trace"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"edges"_s, edges}}}});
    }
    void exited()
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.exited"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"reason"_s, u"done"_s}}}});
    }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.33.0"_s}});
        } else if (m == u"sim.start") {
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, false},
                                  {u"engine"_s, u"lcgamboa"_s}, {u"pins"_s, pinsToReport}});
        } else if (m == u"sim.traceSave") {
            const QJsonObject p = req.value(u"params"_s).toObject();
            saves << p;
            if (!saveError.isEmpty())
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32005}, {u"message"_s, saveError}}}});
            else
                reply(id, QJsonObject{{u"path"_s, p.value(u"path"_s)}, {u"edges"_s, 3}, {u"dropped"_s, 0}});
        } else {
            reply(id, QJsonObject{});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestTrace : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void drawsEdgesAndSavesTheTrace()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        TracePanel panel(&core);
        panel.resize(800, 200);
        panel.show();
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QVERIFY(!panel.saveButton()->isEnabled());

        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_COMPARE(panel.view()->pins(), (QList<int>{2, 4}));
        QVERIFY(panel.saveButton()->isEnabled());

        fake.trace(QJsonArray{edge(1'000'000, 2, true), edge(1'500'000, 2, false), edge(1'600'000, 4, true, true),
                              edge(1'700'000, 9, true)}); // not ours: ignored
        QTRY_COMPARE(panel.view()->edges(2).size(), 2);
        QCOMPARE(panel.view()->edges(4).size(), 1);
        QVERIFY(panel.view()->edges(4)[0].input);
        QVERIFY(panel.view()->edges(9).isEmpty());
        QCOMPARE(panel.view()->now(), 1'600'000);
        panel.spanBox()->setCurrentIndex(1); // 1 ms
        QTest::qWait(20);                    // paints without trouble

        // Saving works after the simulation has ended.
        fake.exited();
        QTRY_VERIFY(panel.statusText().contains(u"can still be saved"_s));
        QVERIFY(panel.saveButton()->isEnabled());
        panel.saveTo(u"/tmp/blink.vcd"_s);
        QTRY_COMPARE(fake.saves.size(), 1);
        QCOMPARE(fake.saves[0].value(u"subscription"_s).toInteger(), 5);
        QCOMPARE(fake.saves[0].value(u"path"_s).toString(), u"/tmp/blink.vcd"_s);
        QTRY_VERIFY(panel.statusText().contains(u"Saved 3 pin changes"_s));

        fake.saveError = u"writing /tmp/blink.vcd: denied"_s;
        panel.saveTo(u"/tmp/blink.vcd"_s);
        QTRY_VERIFY(panel.statusText().contains(u"denied"_s));
    }

    void measuresAndZooms()
    {
        TraceView view;
        view.resize(664, 120); // a 600-pixel plot after the labels
        view.setPins({2});
        view.setSpan(1'000'000);
        view.addEdges(QJsonArray{QJsonObject{{u"gpio"_s, 2}, {u"t"_s, 5'000'000}, {u"level"_s, true}}});
        // Two clicks: A then B; the time between them, and its frequency.
        view.setMarkers(4'000'000, 4'500'000);
        QCOMPARE(view.measurement(), u"A–B: 500 µs (2 kHz)"_s);
        const QPoint at(64 + 300, 60);
        QTest::mouseClick(&view, Qt::LeftButton, {}, at);
        QVERIFY(view.markerA() > 4'000'000 && view.markerB() < 0); // a new A
        QTest::mouseClick(&view, Qt::RightButton, {}, at);
        QVERIFY(view.markerA() < 0);
        QVERIFY(view.measurement().isEmpty());
        // Ctrl+wheel zooms in around a time, which stays where it was.
        QSignalSpy held(&view, &TraceView::held);
        const qint64 t = view.timeAt(at.x());
        view.zoomAround(t, 0.5);
        QCOMPARE(view.span(), 500'000);
        QVERIFY(std::abs(view.timeAt(at.x()) - t) < 2'000);
        QCOMPARE(held.size(), 1);
    }

    void namesPinsFromTheWiring()
    {
        TraceView view;
        view.setPins({2, 4});
        QCOMPARE(view.pinName(2), u"GPIO 2"_s);
        view.setLabels({{2, u"LED anode"_s}});
        QCOMPARE(view.pinName(2), u"2 · LED anode"_s);
        QCOMPARE(view.pinName(4), u"GPIO 4"_s);
    }

    void hidesPins()
    {
        TraceView view;
        view.setPins({2, 4, 5});
        QCOMPARE(view.shownPins(), (QList<int>{2, 4, 5}));
        const int h = view.sizeHint().height();
        view.setPinHidden(4, true);
        QCOMPARE(view.shownPins(), (QList<int>{2, 5}));
        QVERIFY(view.sizeHint().height() < h);
        view.showOnly(5);
        QCOMPARE(view.shownPins(), QList<int>{5});
        // The one row left is the first under the time axis.
        int found = -1;
        for (int y = 0; y < view.sizeHint().height() && found < 0; ++y)
            found = view.pinAt(y);
        QCOMPARE(found, 5);
        QCOMPARE(view.pinAt(0), -1); // the time axis
        view.showAll();
        QCOMPARE(view.shownPins(), (QList<int>{2, 4, 5}));
    }

    void nothingToTraceWithoutPins()
    {
        FakeCore fake(path());
        fake.pinsToReport = {};
        CoreConnection core;
        core.setSocketPath(path());
        TracePanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"blink"_s, u"esp32s3"_s);
        QTRY_VERIFY(panel.statusText().contains(u"No pin access"_s));
        QVERIFY(!panel.saveButton()->isEnabled());
    }
};

QTEST_MAIN(TestTrace)
#include "tst_trace.moc"
