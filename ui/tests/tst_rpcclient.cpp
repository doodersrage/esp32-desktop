// RpcClient and CoreConnection against a fake core on a local socket.

#include "CoreConnection.h"
#include "RpcClient.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

#include <functional>

using namespace Qt::StringLiterals;

// A fake core: calls `respond` for each request line it receives and writes back
// whatever lines it returns.
class FakeCore : public QObject {
    Q_OBJECT
public:
    using Responder = std::function<QList<QJsonObject>(const QJsonObject &request)>;

    explicit FakeCore(const QString &path, Responder respond)
        : m_server(new QLocalServer(this))
        , m_respond(std::move(respond))
    {
        QLocalServer::removeServer(path);
        QVERIFY(m_server->listen(path));
        connect(m_server, &QLocalServer::newConnection, this, [this] {
            QLocalSocket *s = m_server->nextPendingConnection();
            m_last = s;
            connect(s, &QLocalSocket::readyRead, s, [this, s] {
                while (s->canReadLine()) {
                    const QJsonObject req = QJsonDocument::fromJson(s->readLine()).object();
                    for (const QJsonObject &out : m_respond(req))
                        s->write(QJsonDocument(out).toJson(QJsonDocument::Compact) + '\n');
                }
            });
        });
    }

    bool hasClient() const { return !m_last.isNull(); }

    void dropClient()
    {
        if (m_last)
            m_last->abort();
    }

private:
    QLocalServer *m_server;
    Responder m_respond;
    QPointer<QLocalSocket> m_last;
};

static QJsonObject result(const QJsonObject &req, const QJsonValue &r)
{
    return {{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, req.value(u"id"_s)}, {u"result"_s, r}};
}

static QJsonObject error(const QJsonObject &req, int code, const QString &msg)
{
    return {{u"jsonrpc"_s, u"2.0"_s},
            {u"id"_s, req.value(u"id"_s)},
            {u"error"_s, QJsonObject{{u"code"_s, code}, {u"message"_s, msg}}}};
}

class TestRpcClient : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

    // Connect a client and wait until it is connected.
    void connectClient(RpcClient &c)
    {
        // QLocalSocket may emit connected() inside connectToServer(), so poll the state.
        c.connectToCore(path());
        QTRY_VERIFY_WITH_TIMEOUT(c.isConnected(), 2000);
    }

private slots:
    void resultsAndErrors()
    {
        FakeCore core(path(), [](const QJsonObject &req) -> QList<QJsonObject> {
            if (req.value(u"method"_s).toString() == u"test.echo")
                return {result(req, req.value(u"params"_s))};
            return {error(req, -32601, u"unknown method"_s)};
        });
        RpcClient c;
        connectClient(c);

        RpcReply got;
        int calls = 0;
        c.call(u"test.echo"_s, {{u"a"_s, 1}}, this, [&](const RpcReply &r) { got = r; ++calls; });
        QTRY_COMPARE(calls, 1);
        QVERIFY(got.ok);
        QCOMPARE(got.result.toObject().value(u"a"_s).toInt(), 1);

        c.call(u"test.nope"_s, {}, this, [&](const RpcReply &r) { got = r; ++calls; });
        QTRY_COMPARE(calls, 2);
        QVERIFY(!got.ok);
        QCOMPARE(got.errorCode, -32601);
        QCOMPARE(got.errorMessage, u"unknown method"_s);
    }

    void matchesRepliesById()
    {
        // The fake core answers the first request only after the second arrives,
        // so replies come back in reverse order.
        QJsonObject held;
        FakeCore core(path(), [&](const QJsonObject &req) -> QList<QJsonObject> {
            if (held.isEmpty()) {
                held = req;
                return {};
            }
            return {result(req, u"second"_s), result(held, u"first"_s)};
        });
        RpcClient c;
        connectClient(c);
        QString a, b;
        c.call(u"x"_s, {}, this, [&](const RpcReply &r) { a = r.result.toString(); });
        c.call(u"y"_s, {}, this, [&](const RpcReply &r) { b = r.result.toString(); });
        QTRY_COMPARE(a, u"first"_s);
        QCOMPARE(b, u"second"_s);
    }

    void emitsNotifications()
    {
        FakeCore core(path(), [](const QJsonObject &req) -> QList<QJsonObject> {
            const QJsonObject note{{u"jsonrpc"_s, u"2.0"_s},
                                   {u"method"_s, u"device.added"_s},
                                   {u"params"_s, QJsonObject{{u"subscription"_s, 3}}}};
            return {note, result(req, QJsonValue())};
        });
        RpcClient c;
        connectClient(c);
        QSignalSpy notes(&c, &RpcClient::notification);
        c.call(u"x"_s, {}, this, [](const RpcReply &) {});
        QTRY_COMPARE(notes.size(), 1);
        QCOMPARE(notes.at(0).at(0).toString(), u"device.added"_s);
        QCOMPARE(notes.at(0).at(1).toJsonObject().value(u"subscription"_s).toInt(), 3);
    }

    void failsPendingCallsOnDisconnect()
    {
        FakeCore core(path(), [](const QJsonObject &) { return QList<QJsonObject>{}; });
        RpcClient c;
        connectClient(c);
        RpcReply got;
        bool called = false;
        c.call(u"never.answered"_s, {}, this, [&](const RpcReply &r) { got = r; called = true; });
        QSignalSpy down(&c, &RpcClient::disconnected);
        QTRY_VERIFY(core.hasClient());
        core.dropClient();
        QVERIFY(down.wait(2000));
        QVERIFY(called);
        QCOMPARE(got.errorCode, RpcClient::DisconnectedError);
    }

    void callWhileDisconnectedFailsAsynchronously()
    {
        RpcClient c;
        bool called = false;
        RpcReply got;
        c.call(u"x"_s, {}, this, [&](const RpcReply &r) { got = r; called = true; });
        QVERIFY(!called); // never synchronously
        QTRY_VERIFY(called);
        QCOMPARE(got.errorCode, RpcClient::DisconnectedError);
    }

    void skipsHandlerWhenContextIsGone()
    {
        FakeCore core(path(), [](const QJsonObject &req) { return QList<QJsonObject>{result(req, 1)}; });
        RpcClient c;
        connectClient(c);
        bool called = false;
        auto *context = new QObject(this);
        c.call(u"x"_s, {}, context, [&](const RpcReply &) { called = true; });
        delete context;
        QTest::qWait(200);
        QVERIFY(!called);
    }

    void handshakeAcceptsCompatibleCore()
    {
        FakeCore core(path(), [](const QJsonObject &req) -> QList<QJsonObject> {
            return {result(req, QJsonObject{{u"name"_s, u"esp32benchd"_s},
                                            {u"version"_s, u"9.9.9"_s},
                                            {u"protocol"_s, u"0.99.2"_s}})}; // a newer core is fine
        });
        CoreConnection conn;
        conn.setSocketPath(path());
        conn.start();
        QTRY_COMPARE(conn.state(), CoreConnection::State::Connected);
        QCOMPARE(conn.coreVersion(), u"9.9.9"_s);
    }

    void handshakeRejectsAnOlderCore()
    {
        // A core left running from before an upgrade: same major, older minor.
        FakeCore core(path(), [](const QJsonObject &req) -> QList<QJsonObject> {
            return {result(req, QJsonObject{{u"name"_s, u"esp32benchd"_s},
                                            {u"version"_s, u"0.0.1"_s},
                                            {u"protocol"_s, u"0.17.0"_s}})};
        });
        CoreConnection conn;
        conn.setSocketPath(path());
        conn.start();
        QTRY_COMPARE(conn.state(), CoreConnection::State::Incompatible);
        QVERIFY(conn.lastError().contains(u"older core (protocol 0.17.0)"_s));
        QVERIFY(conn.lastError().contains(u"pkill -x esp32benchd"_s));
    }

    void handshakeRejectsOtherMajorVersion()
    {
        FakeCore core(path(), [](const QJsonObject &req) -> QList<QJsonObject> {
            return {result(req, QJsonObject{{u"name"_s, u"esp32benchd"_s},
                                            {u"version"_s, u"2.0.0"_s},
                                            {u"protocol"_s, u"1.0.0"_s}})};
        });
        CoreConnection conn;
        conn.setSocketPath(path());
        conn.start();
        QTRY_COMPARE(conn.state(), CoreConnection::State::Incompatible);
        QVERIFY(conn.lastError().contains(u"1.0.0"_s));
    }

    void watchDevicesFollowsItsSubscription()
    {
        FakeCore core(path(), [](const QJsonObject &req) -> QList<QJsonObject> {
            const QString m = req.value(u"method"_s).toString();
            if (m == u"core.hello")
                return {result(req, QJsonObject{{u"name"_s, u"esp32benchd"_s},
                                                {u"version"_s, u"0.0.1"_s},
                                                {u"protocol"_s, u"0.28.0"_s}})};
            if (m == u"device.watch") {
                const QJsonArray ports{QJsonObject{{u"path"_s, u"/dev/ttyUSB0"_s}}};
                auto changed = [](int sub, const QString &path) {
                    return QJsonObject{
                        {u"jsonrpc"_s, u"2.0"_s},
                        {u"method"_s, u"device.changed"_s},
                        {u"params"_s, QJsonObject{{u"subscription"_s, sub},
                                                  {u"ports"_s, QJsonArray{QJsonObject{{u"path"_s, path}}}}}}};
                };
                // The answer, then an event for another subscription (ignored), then
                // one for this subscription.
                return {result(req, QJsonObject{{u"subscription"_s, 5}, {u"ports"_s, ports}}),
                        changed(6, u"/dev/other"_s), changed(5, u"/dev/ttyACM0"_s)};
            }
            return {error(req, -32601, u"unknown"_s)};
        });
        CoreConnection conn;
        conn.setSocketPath(path());
        QSignalSpy listed(&conn, &CoreConnection::devicesListed);
        conn.start();
        QTRY_COMPARE(conn.state(), CoreConnection::State::Connected);
        conn.watchDevices();
        QTRY_COMPARE(listed.size(), 2);
        QTest::qWait(100);
        QCOMPARE(listed.size(), 2); // the event for subscription 6 was ignored
        auto first = listed.at(0).at(0).toJsonArray();
        auto second = listed.at(1).at(0).toJsonArray();
        QCOMPARE(first.at(0).toObject().value(u"path"_s).toString(), u"/dev/ttyUSB0"_s);
        QCOMPARE(second.at(0).toObject().value(u"path"_s).toString(), u"/dev/ttyACM0"_s);
    }

    void staysDisconnectedWithoutCore()
    {
        CoreConnection conn;
        conn.setSocketPath(m_dir.filePath(u"nobody-here.sock"_s));
        conn.start();
        QTest::qWait(300);
        QCOMPARE(conn.state(), CoreConnection::State::Disconnected);
        QVERIFY(!conn.lastError().isEmpty());
    }

    void autoStartTriesOnceAndSaysWhy()
    {
        // A daemon path that cannot run: the start fails, and later retries do not
        // try again.
        qputenv("ESP32BENCH_DAEMON", m_dir.filePath(u"no-such-esp32benchd"_s).toLocal8Bit());
        CoreConnection conn;
        conn.setSocketPath(m_dir.filePath(u"nobody-here-either.sock"_s));
        conn.setAutoStart(true);
        QSignalSpy failed(&conn, &CoreConnection::autoStartFailed);
        conn.start();
        QTRY_COMPARE(failed.size(), 1);
        QVERIFY(failed.at(0).at(0).toString().contains(u"no-such-esp32benchd"_s));
        QTest::qWait(2500); // more than one retry
        QCOMPARE(failed.size(), 1);
        QCOMPARE(conn.state(), CoreConnection::State::Disconnected);
        qunsetenv("ESP32BENCH_DAEMON");
    }
};

QTEST_MAIN(TestRpcClient)
#include "tst_rpcclient.moc"
