// RpcClient and CoreConnection against a fake core on a local socket.

#include "CoreConnection.h"
#include "RpcClient.h"

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
                                            {u"protocol"_s, u"0.4.2"_s}})};
        });
        CoreConnection conn;
        conn.setSocketPath(path());
        conn.start();
        QTRY_COMPARE(conn.state(), CoreConnection::State::Connected);
        QCOMPARE(conn.coreVersion(), u"9.9.9"_s);
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

    void staysDisconnectedWithoutCore()
    {
        CoreConnection conn;
        conn.setSocketPath(m_dir.filePath(u"nobody-here.sock"_s));
        conn.start();
        QTest::qWait(300);
        QCOMPARE(conn.state(), CoreConnection::State::Disconnected);
        QVERIFY(!conn.lastError().isEmpty());
    }
};

QTEST_MAIN(TestRpcClient)
#include "tst_rpcclient.moc"
