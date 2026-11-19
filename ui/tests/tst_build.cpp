// The build flow against a fake core: consent refusal, retry with the token, early
// events, and MainWindow asking the user (answered here without a dialog).

#include "BuildPanel.h"
#include "CoreConnection.h"
#include "MainWindow.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QStandardPaths>
#include <QTest>

using namespace Qt::StringLiterals;

// A fake core that knows core.hello and build.start. build.start without the right
// token is refused with -32002; with it, the build's events are sent *before* the
// reply, which docs/rpc.md allows, to check the UI waits for its job id.
class FakeCore : public QObject {
    Q_OBJECT
public:
    QStringList buildRequests; // the consent value of each build.start, or "-"

    explicit FakeCore(const QString &path)
        : m_server(new QLocalServer(this))
    {
        QLocalServer::removeServer(path);
        QVERIFY(m_server->listen(path));
        connect(m_server, &QLocalServer::newConnection, this, [this] {
            QLocalSocket *s = m_server->nextPendingConnection();
            connect(s, &QLocalSocket::readyRead, s, [this, s] {
                while (s->canReadLine())
                    handle(s, QJsonDocument::fromJson(s->readLine()).object());
            });
        });
    }

private:
    static void send(QLocalSocket *s, const QJsonObject &o)
    {
        s->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n');
    }
    static QJsonObject event(const QString &method, QJsonObject params)
    {
        params[u"subscription"_s] = 9;
        return {{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, method}, {u"params"_s, params}};
    }

    void handle(QLocalSocket *s, const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString method = req.value(u"method"_s).toString();
        const QJsonObject params = req.value(u"params"_s).toObject();
        const QJsonObject plan{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s},
                               {u"target"_s, u"esp32"_s}, {u"fqbn"_s, u"esp32:esp32:esp32:FlashMode=dio"_s},
                               {u"codeSha256"_s, QString(64, u'a')},
                               {u"install"_s, QJsonArray{u"Adafruit NeoPixel"_s}}};
        if (method == u"core.hello") {
            send(s, {{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                     {u"result"_s, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s},
                                               {u"protocol"_s, u"0.23.0"_s}}}});
        } else if (method == u"build.start") {
            const QString consent = params.value(u"consent"_s).toString();
            buildRequests << (consent.isEmpty() ? u"-"_s : consent);
            if (consent != u"tok123") {
                send(s, {{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                         {u"error"_s, QJsonObject{{u"code"_s, -32002}, {u"message"_s, u"ask first"_s},
                                                  {u"data"_s, QJsonObject{{u"token"_s, u"tok123"_s},
                                                                          {u"plan"_s, plan}}}}}});
                return;
            }
            send(s, event(u"build.log"_s, {{u"stream"_s, u"stdout"_s}, {u"line"_s, u"Installed Adafruit NeoPixel"_s}}));
            send(s, event(u"build.log"_s, {{u"stream"_s, u"stdout"_s}, {u"line"_s, u"Sketch uses 1234 bytes"_s}}));
            send(s, event(u"build.finished"_s, {{u"ok"_s, true}, {u"image"_s, u"/tmp/blink.bin"_s}}));
            send(s, {{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                     {u"result"_s, QJsonObject{{u"job"_s, 9}, {u"plan"_s, plan}}}});
        }
    }

    QLocalServer *m_server;
};

// MainWindow with a scripted answer to the consent question.
class AnsweringWindow : public MainWindow {
public:
    using MainWindow::MainWindow;
    bool answer = false;
    int asked = 0;
    QJsonObject askedPlan;
    bool confirmBuild(const QString &, const QString &, const QJsonObject &plan) override
    {
        ++asked;
        askedPlan = plan;
        return answer;
    }
};

class TestBuild : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    // Keep QSettings away from the real configuration.
    void initTestCase() { QStandardPaths::setTestModeEnabled(true); }

    void consentThenBuildWithEarlyEvents()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        QSignalSpy consent(&core, &CoreConnection::buildConsentRequired);
        core.startBuild(u"blink"_s, u"esp32"_s);
        QTRY_COMPARE(consent.size(), 1);
        QCOMPARE(consent.at(0).at(2).toString(), u"tok123"_s);
        QCOMPARE(consent.at(0).at(3).toJsonObject().value(u"install"_s).toArray().at(0).toString(),
                 u"Adafruit NeoPixel"_s);
        QVERIFY(!core.isBuilding());

        QSignalSpy started(&core, &CoreConnection::buildStarted);
        QSignalSpy logs(&core, &CoreConnection::buildLog);
        QSignalSpy finished(&core, &CoreConnection::buildFinished);
        core.startBuild(u"blink"_s, u"esp32"_s, u"tok123"_s);
        QTRY_COMPARE(finished.size(), 1);
        QCOMPARE(started.size(), 1);
        // The events came before the reply but are delivered, in order, after it.
        QCOMPARE(logs.size(), 2);
        QCOMPARE(logs.at(0).at(1).toString(), u"Installed Adafruit NeoPixel"_s);
        QCOMPARE(logs.at(1).at(1).toString(), u"Sketch uses 1234 bytes"_s);
        QVERIFY(finished.at(0).at(0).toBool());
        QCOMPARE(finished.at(0).at(2).toString(), u"/tmp/blink.bin"_s);
        QVERIFY(!core.isBuilding());
        QCOMPARE(fake.buildRequests, (QStringList{u"-"_s, u"tok123"_s}));
    }

    void windowAsksAndBuildsOnlyIfTheUserAgrees()
    {
        for (const bool agree : {false, true}) {
            FakeCore fake(path());
            CoreConnection core;
            core.setSocketPath(path());
            AnsweringWindow window(&core);
            window.answer = agree;
            core.start();
            QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

            QSignalSpy finished(&core, &CoreConnection::buildFinished);
            core.startBuild(u"blink"_s, u"esp32"_s);
            QTRY_COMPARE(window.asked, 1);
            QCOMPARE(window.askedPlan.value(u"codeSha256"_s).toString(), QString(64, u'a'));
            if (agree) {
                QTRY_COMPARE(finished.size(), 1);
                QCOMPARE(fake.buildRequests, (QStringList{u"-"_s, u"tok123"_s}));
                QVERIFY(window.buildPanel()->statusText().contains(u"/tmp/blink.bin"_s));
                QVERIFY(window.buildPanel()->logText().contains(u"Sketch uses 1234 bytes"_s));
            } else {
                QTest::qWait(200);
                // Declining sends nothing more, so nothing is built.
                QCOMPARE(fake.buildRequests, QStringList{u"-"_s});
                QCOMPARE(finished.size(), 0);
            }
        }
    }
};

QTEST_MAIN(TestBuild)
#include "tst_build.moc"
