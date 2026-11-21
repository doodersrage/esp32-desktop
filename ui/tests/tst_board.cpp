// Flash, inspect and erase from the UI against a fake core: the serial monitor gets
// out of the way and comes back, consent and confirmation are asked for.

#include "BoardInfoDialog.h"
#include "BuildPanel.h"
#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "MainWindow.h"
#include "SimulatorPanel.h"
#include "MessageBar.h"
#include "SerialMonitor.h"

#include <QComboBox>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QStandardPaths>
#include <QTemporaryDir>
#include <QToolButton>
#include <QMenu>
#include <QAction>
#include <QLabel>
#include <QJsonObject>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

// Records the calls it gets. board.flash is refused for consent until it carries
// "tok"; board jobs send their events before the reply.
class FakeCore : public QObject {
    Q_OBJECT
public:
    int openedBaud = 0;
    QStringList calls; // "method" or "method consent=..." etc.

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
    void event(int sub, const QString &m, QJsonObject p)
    {
        p[u"subscription"_s] = sub;
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, m}, {u"params"_s, p}});
    }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        const QJsonArray ports{QJsonObject{{u"path"_s, u"/dev/ttyACM0"_s}}};
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.30.0"_s}});
            return;
        }
        if (m == u"device.watch") {
            reply(id, QJsonObject{{u"subscription"_s, 1}, {u"ports"_s, ports}});
            return;
        }
        if (m == u"library.list") {
            reply(id, QJsonObject{{u"available"_s, true}, {u"source"_s, u"x"_s}, {u"projects"_s, QJsonArray{}}});
            return;
        }
        if (m == u"toolchain.status") {
            reply(id, QJsonObject{{u"components"_s, QJsonArray{}}});
            return;
        }
        calls << (m == u"board.flash" ? u"board.flash consent=%1"_s.arg(p.value(u"consent"_s).toString())
                  : m == u"board.erase" ? u"board.erase confirm=%1"_s.arg(p.value(u"confirm"_s).toBool() ? u"true"_s : u"false"_s)
                                        : m);
        if (m == u"build.start") {
            event(8, u"build.finished"_s, {{u"ok"_s, true}, {u"image"_s, u"/tmp/blink.bin"_s}});
            reply(id, QJsonObject{{u"job"_s, 8}, {u"plan"_s, QJsonObject{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s}}}});
        } else if (m == u"serial.open") {
            openedBaud = p.value(u"baud"_s).toInt();
            reply(id, QJsonObject{{u"subscription"_s, 4}});
        } else if (m == u"core.unsubscribe") {
            reply(id, QJsonObject{});
        } else if (m == u"board.flash" && p.value(u"consent"_s).toString() != u"tok") {
            send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                  {u"error"_s, QJsonObject{{u"code"_s, -32002}, {u"message"_s, u"ask"_s},
                                           {u"data"_s, QJsonObject{{u"token"_s, u"tok"_s},
                                                                   {u"plan"_s, QJsonObject{{u"project"_s, u"blink"_s},
                                                                                           {u"board"_s, u"esp32"_s},
                                                                                           {u"bytes"_s, 1000}}}}}}}});
        } else if (m.startsWith(u"board.")) {
            event(9, u"board.progress"_s, {{u"stage"_s, u"connecting"_s}});
            QJsonObject done{{u"ok"_s, true}};
            if (m == u"board.inspect") {
                done[u"info"_s] = QJsonObject{{u"chip"_s, u"esp32s3"_s}, {u"revision"_s, u"v0.2"_s}, {u"crystalMhz"_s, 40},
                                              {u"flashBytes"_s, 8388608}, {u"features"_s, QJsonArray{u"WiFi"_s}},
                                              {u"security"_s, QJsonObject{{u"secureBoot"_s, false}, {u"flashEncryption"_s, false},
                                                                          {u"secureDownload"_s, false}}}};
                done[u"partitions"_s] = QJsonArray{QJsonObject{{u"label"_s, u"nvs"_s}, {u"type"_s, u"data"_s},
                                                               {u"subtype"_s, u"nvs"_s}, {u"offset"_s, 0x9000},
                                                               {u"size"_s, 0x5000}, {u"encrypted"_s, false}}};
            }
            if (m == u"board.flash") {
                event(9, u"board.progress"_s, {{u"stage"_s, u"writing"_s}, {u"written"_s, 500}, {u"total"_s, 1000}});
                done[u"written"_s] = 1000;
            }
            event(9, u"board.finished"_s, done);
            reply(id, QJsonObject{{u"job"_s, 9}});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class ScriptedWindow : public MainWindow {
public:
    using MainWindow::MainWindow;
    bool flashAnswer = true;
    bool eraseAnswer = false;
    int flashAsked = 0, eraseAsked = 0;
    bool confirmFlash(const QString &, const QJsonObject &) override
    {
        ++flashAsked;
        return flashAnswer;
    }
    bool confirmErase(const QString &) override
    {
        ++eraseAsked;
        return eraseAnswer;
    }
    bool confirmBuild(const QString &, const QString &, const QJsonObject &) override { return true; }
};

class TestBoard : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void initTestCase() { QStandardPaths::setTestModeEnabled(true); }

    void flashesTheBuildAroundTheSerialMonitor()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        ScriptedWindow window(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        // A monitor is open on the board, and a build has succeeded.
        window.serialMonitor()->openPort(u"/dev/ttyACM0"_s);
        QTRY_VERIFY(core.isSerialOpen());
        core.startBuild(u"blink"_s, u"esp32"_s);
        BuildPanel *build = window.buildPanel();
        QTRY_VERIFY(build->flashAction()->isEnabled());
        QCOMPARE(build->portPicker()->currentData().toString(), u"/dev/ttyACM0"_s);

        fake.calls.clear();
        build->flashAction()->trigger();
        // The monitor lets go of the port, consent is asked, the flash runs with the
        // token, and the monitor comes back on the same port.
        QTRY_VERIFY(fake.calls.contains(u"board.flash consent=tok"_s));
        QCOMPARE(window.flashAsked, 1);
        QTRY_VERIFY(build->statusText().contains(u"Flashed 1000 bytes"_s));
        QTRY_VERIFY(core.isSerialOpen());
        QCOMPARE(fake.calls.mid(0, 3),
                 (QStringList{u"core.unsubscribe"_s, u"board.flash consent="_s, u"board.flash consent=tok"_s}));
        QCOMPARE(fake.calls.last(), u"serial.open"_s);

        // The monitor comes back at the flashed project's baud rate, here an unusual one.
        // (The project view shows it too, so it is the blink the last build was of.)
        const QJsonObject fw{{u"board"_s, u"esp32"_s}, {u"pins"_s, QJsonArray{}}, {u"wiring"_s, QJsonArray{}},
                                {u"code"_s, u"void setup() {}"_s}, {u"sample"_s, QJsonArray{}}};
        emit core.projectLoaded(QJsonObject{
            {u"project"_s, QJsonObject{{u"id"_s, u"blink"_s}, {u"name"_s, u"Blink"_s}, {u"firmware"_s, u"sketch"_s},
                                       {u"baud"_s, 74880}, {u"builds"_s, QJsonArray{fw}}}},
            {u"boards"_s, QJsonArray{QJsonObject{{u"id"_s, u"esp32"_s}, {u"name"_s, u"ESP32"_s}, {u"chip"_s, u"esp32"_s}}}}});
        QTRY_VERIFY(build->flashAction()->isEnabled());
        fake.calls.clear();
        build->flashAction()->trigger();
        QTRY_VERIFY2(fake.calls.contains(u"board.flash consent=tok"_s), qPrintable(fake.calls.join(u", "_s)));
        QTRY_COMPARE(fake.calls.last(), u"serial.open"_s);
        QCOMPARE(fake.openedBaud, 74880);
    }

    void declinedFlashSendsNothingMore()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        ScriptedWindow window(&core);
        window.flashAnswer = false;
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startBuild(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(window.buildPanel()->flashAction()->isEnabled());
        fake.calls.clear();
        window.buildPanel()->flashAction()->trigger();
        QTRY_COMPARE(window.flashAsked, 1);
        QTest::qWait(200);
        QCOMPARE(fake.calls, QStringList{u"board.flash consent="_s});
    }

    void explainsWhatTheBoardPrinted()
    {
        CoreConnection core;
        ScriptedWindow window(&core);
        // A problem (a crash): in the message bar, with the site's page.
        emit core.outputExplained(u"sim"_s, QJsonArray{QJsonObject{
            {u"key"_s, u"panic-load"_s}, {u"text"_s, u"The code read from an address it may not touch."_s},
            {u"level"_s, u"problem"_s}, {u"url"_s, u"https://esp32bench.dev/errors/guru-meditation-loadprohibited/"_s}}});
        QVERIFY(window.messageBar()->text().contains(u"may not touch"_s));
        QVERIFY(window.messageBar()->findChild<QLabel *>(QString(), Qt::FindDirectChildrenOnly) != nullptr);
        bool linked = false;
        for (QLabel *l : window.messageBar()->findChildren<QLabel *>())
            linked |= l->text().contains(u"href=\"https://esp32bench.dev/errors/guru-meditation-loadprohibited/\""_s);
        QVERIFY(linked);
        // A note (a normal reset): in the console it came from, not the bar.
        window.messageBar()->hide();
        emit core.outputExplained(u"sim"_s, QJsonArray{QJsonObject{
            {u"key"_s, u"rst-POWERON_RESET"_s}, {u"text"_s, u"POWERON_RESET: Normal start."_s}, {u"level"_s, u"info"_s}}});
        QVERIFY(window.messageBar()->text().isEmpty());
        QVERIFY(window.simulator()->outputText().contains(u"Note: POWERON_RESET: Normal start."_s));
    }

    void inspectShowsTheBoardAndEraseAsksFirst()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        ScriptedWindow window(&core);
        core.start();
        QTRY_COMPARE(window.devicesPanel()->tree()->topLevelItemCount(), 1);

        emit window.devicesPanel()->inspectRequested(u"/dev/ttyACM0"_s);
        QTRY_VERIFY(window.boardInfo()->isVisible());
        QVERIFY(window.boardInfo()->summaryText().contains(u"ESP32-S3"_s));
        QCOMPARE(window.boardInfo()->partitionTree()->topLevelItemCount(), 1);
        QCOMPARE(window.boardInfo()->partitionTree()->topLevelItem(0)->text(3), u"0x009000"_s);
        window.boardInfo()->hide();

        fake.calls.clear();
        window.eraseAnswer = false;
        emit window.devicesPanel()->eraseRequested(u"/dev/ttyACM0"_s);
        QCOMPARE(window.eraseAsked, 1);
        QTest::qWait(100);
        QVERIFY(fake.calls.isEmpty());
        window.eraseAnswer = true;
        emit window.devicesPanel()->eraseRequested(u"/dev/ttyACM0"_s);
        QTRY_COMPARE(fake.calls, QStringList{u"board.erase confirm=true"_s});

        // The toolbar's Board menu does the same for the port chosen there.
        QTRY_VERIFY(window.boardMenuButton()->isEnabled());
        QCOMPARE(window.buildPanel()->portPicker()->currentData().toString(), u"/dev/ttyACM0"_s);
        const auto action = [&](const QString &text) -> QAction * {
            for (QAction *a : window.boardMenuButton()->menu()->actions()) {
                if (a->text() == text)
                    return a;
            }
            return nullptr;
        };
        QVERIFY(action(u"Inspect board…"_s));
        QVERIFY(action(u"Erase flash…"_s));
        fake.calls.clear();
        action(u"Erase flash…"_s)->trigger();
        QCOMPARE(window.eraseAsked, 3);
        QTRY_COMPARE(fake.calls, QStringList{u"board.erase confirm=true"_s});
    }
};

QTEST_MAIN(TestBoard)
#include "tst_board.moc"
