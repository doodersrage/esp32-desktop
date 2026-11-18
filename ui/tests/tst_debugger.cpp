// DebugPanel, BuildPanel's Debug button and CoreConnection's debug calls against a
// fake core.

#include "BuildPanel.h"
#include "CoreConnection.h"
#include "DebugPanel.h"
#include "MainWindow.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QLineEdit>
#include <QListWidget>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QDockWidget>
#include <QPushButton>
#include <QTemporaryDir>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

class FakeCore : public QObject {
    Q_OBJECT
public:
    QList<QJsonObject> calls; // method and params of each call after the handshake
    bool debugEnabled = true;

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

    QStringList methods() const
    {
        QStringList out;
        for (const QJsonObject &c : calls)
            out << c.value(u"method"_s).toString();
        return out;
    }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void event(const QString &method, QJsonObject params)
    {
        params.insert(u"subscription"_s, 5);
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, method}, {u"params"_s, params}});
    }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.20.0"_s}});
            return;
        }
        if (m == u"device.list" || m == u"device.watch") {
            reply(id, QJsonObject{{u"ports"_s, QJsonArray{}}, {u"subscription"_s, 1}});
            return;
        }
        calls << QJsonObject{{u"method"_s, m}, {u"params"_s, p}};
        const QJsonObject setup{{u"addr"_s, u"0x400d1a2c"_s}, {u"func"_s, u"setup"_s},
                                {u"file"_s, u"/b/blink/esp32/sketch/blink.ino"_s}, {u"line"_s, 4}};
        if (m == u"sim.start") {
            const bool debug = p.value(u"debug"_s).toBool();
            if (debug) // the attach stop can come before the reply
                event(u"debug.stopped"_s, {{u"reason"_s, u"paused"_s}, {u"frame"_s, QJsonObject{{u"addr"_s, u"0x40000400"_s}}}});
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, debug}});
        } else if (m == u"debug.break") {
            if (p.value(u"location"_s).toString() == u"nope") {
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32000},
                                               {u"message"_s, u"Function \"nope\" not defined."_s}}}});
                return;
            }
            QJsonObject bp = setup;
            bp.insert(u"number"_s, 1);
            reply(id, QJsonObject{{u"breakpoint"_s, bp}});
        } else if (m == u"debug.control") {
            reply(id, QJsonObject{});
            const QString a = p.value(u"action"_s).toString();
            if (a == u"continue") {
                event(u"debug.running"_s, {});
                event(u"debug.stopped"_s, {{u"reason"_s, u"breakpoint-hit"_s}, {u"breakpoint"_s, 1}, {u"frame"_s, setup}});
            }
        } else if (m == u"debug.inspect") {
            reply(id, QJsonObject{
                          {u"frames"_s, QJsonArray{setup, QJsonObject{{u"addr"_s, u"0x400d2b40"_s}, {u"func"_s, u"loopTask"_s}}}},
                          {u"registers"_s, QJsonArray{QJsonObject{{u"name"_s, u"pc"_s}, {u"value"_s, u"0x400d1a2c"_s}},
                                                      QJsonObject{{u"name"_s, u"ar0"_s}, {u"value"_s, u"0x800d2b44"_s}}}},
                      });
        } else {
            reply(id, QJsonObject{});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestDebugger : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void breaksInspectsAndContinues()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QVERIFY(!panel.continueButton()->isEnabled());

        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_VERIFY(core.isDebugging());
        QCOMPARE(fake.calls.first().value(u"params"_s).toObject().value(u"debug"_s), QJsonValue(true));
        // The stop that came before the reply was not lost.
        QTRY_VERIFY(panel.statusText().contains(u"before the firmware starts"_s));
        QVERIFY(panel.continueButton()->isEnabled());
        QVERIFY(panel.addButton()->isEnabled());
        QVERIFY(!panel.pauseButton()->isEnabled());

        panel.locationInput()->setText(u"nope"_s);
        QTest::keyClick(panel.locationInput(), Qt::Key_Return);
        QTRY_VERIFY(panel.statusText().contains(u"not defined"_s));
        QCOMPARE(panel.breakpoints()->count(), 0);

        panel.locationInput()->setText(u"setup"_s);
        panel.addButton()->click();
        QTRY_COMPARE(panel.breakpoints()->count(), 1);
        QCOMPARE(panel.breakpoints()->item(0)->text(), u"1: setup (blink.ino, line 4)"_s);
        QVERIFY(panel.locationInput()->text().isEmpty());

        panel.continueButton()->click();
        QTRY_VERIFY(panel.statusText().contains(u"Stopped at breakpoint 1 in setup (blink.ino, line 4)"_s));
        QTRY_COMPARE(panel.stack()->topLevelItemCount(), 2);
        QCOMPARE(panel.stack()->topLevelItem(0)->text(1), u"blink.ino:4"_s);
        QCOMPARE(panel.stack()->topLevelItem(1)->text(0), u"loopTask"_s);
        QCOMPARE(panel.registers()->topLevelItem(0)->text(1), u"0x400d1a2c"_s);
        QCOMPARE(panel.breakpoints()->currentRow(), 0);

        panel.removeButton()->click();
        QTRY_COMPARE(panel.breakpoints()->count(), 0);
        const QJsonObject del = fake.calls.last();
        QCOMPARE(del.value(u"method"_s).toString(), u"debug.delete"_s);
        QCOMPARE(del.value(u"params"_s).toObject().value(u"breakpoint"_s).toInt(), 1);

        panel.stepOverButton()->click();
        QTRY_VERIFY(fake.calls.last().value(u"params"_s).toObject().value(u"action"_s) == u"next"_s);

        core.stopSim();
        QTRY_VERIFY(!core.isDebugging());
        QVERIFY(!panel.continueButton()->isEnabled());
        QVERIFY(!panel.addButton()->isEnabled());
    }

    void debuggerSitsBesideTheConsole()
    {
        // A private config directory: MainWindow restores and saves its layout.
        QTemporaryDir config;
        qputenv("XDG_CONFIG_HOME", config.path().toLocal8Bit());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        window.show();
        auto *sim = window.findChild<QDockWidget *>(u"simDock"_s);
        auto *dbg = window.findChild<QDockWidget *>(u"debugDock"_s);
        QVERIFY(sim && dbg);
        emit core.debugStarted();
        QVERIFY(dbg->isVisible());
        QVERIFY(sim->isVisible());
        QVERIFY(!window.tabifiedDockWidgets(dbg).contains(sim));

        // A layout saved with the debugger tabbed over the console is fixed on start.
        window.tabifyDockWidget(sim, dbg);
        QVERIFY(window.tabifiedDockWidgets(dbg).contains(sim));
        emit core.debugStarted();
        QVERIFY(!window.tabifiedDockWidgets(dbg).contains(sim));
        QVERIFY(dbg->isVisible() && sim->isVisible());
    }

    void buildPanelOffersDebugging()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        BuildPanel build(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QVERIFY(!build.debugButton()->isEnabled()); // nothing built yet
        // A plain run does not ask for the debugger.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(!core.isDebugging());
        QCOMPARE(fake.calls.first().value(u"params"_s).toObject().value(u"debug"_s), QJsonValue(false));
    }
};

QTEST_MAIN(TestDebugger)
#include "tst_debugger.moc"
