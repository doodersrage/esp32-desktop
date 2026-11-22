// DebugPanel, BuildPanel's Debug button and CoreConnection's debug calls against a
// fake core.

#include "BacktraceDialog.h"
#include "BuildPanel.h"
#include "CoreConnection.h"
#include "DebugPanel.h"
#include "DiskSpaceDialog.h"
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
#include <QPlainTextEdit>
#include <QSignalSpy>
#include <QCheckBox>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

class FakeCore : public QObject {
    Q_OBJECT
public:
    QList<QJsonObject> calls; // method and params of each call after the handshake
    bool debugEnabled = true;
    QJsonArray kept{u"blink.ino:7"_s}; // breakpoints.get's answer for any project
    QJsonArray watches; // watches.get's
    int inspections = 0; // debug.inspect calls; each says pin is one more
    int hits = 0;        // times breakpoint 1 was reached
    QList<QJsonObject> assignedTo; // debug.assign params
    bool cleaned = false;
    QJsonObject cleanedWith; // cache.clean params
    void stopAtWatch()
    {
        event(u"debug.stopped"_s, {{u"reason"_s, u"watchpoint-trigger"_s}, {u"breakpoint"_s, m_next},
                                   {u"frame"_s, QJsonObject{{u"addr"_s, u"0x400d1a2c"_s}, {u"func"_s, u"loop"_s}}},
                                   {u"watch"_s, QJsonObject{{u"expression"_s, u"count"_s}, {u"old"_s, u"1"_s}, {u"new"_s, u"2"_s}}}});
    }

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
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.47.0"_s}});
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
            bp.insert(u"number"_s, ++m_next);
            if (p.contains(u"condition"_s))
                bp.insert(u"condition"_s, p.value(u"condition"_s));
            reply(id, QJsonObject{{u"breakpoint"_s, bp}});
        } else if (m == u"debug.control") {
            reply(id, QJsonObject{});
            const QString a = p.value(u"action"_s).toString();
            if (a == u"continue") {
                event(u"debug.running"_s, {});
                event(u"debug.stopped"_s,
                      {{u"reason"_s, u"breakpoint-hit"_s}, {u"breakpoint"_s, 1}, {u"frame"_s, setup}, {u"hits"_s, ++hits}});
            } else if (a == u"runTo") {
                // At GDB's temporary breakpoint, number 9: not one of the list.
                QJsonObject at = setup;
                at.insert(u"line"_s, 12);
                event(u"debug.running"_s, {});
                event(u"debug.stopped"_s, {{u"reason"_s, u"breakpoint-hit"_s}, {u"breakpoint"_s, 9}, {u"frame"_s, at}});
            }
        } else if (m == u"debug.evaluate") {
            const QString e = p.value(u"expression"_s).toString();
            if (e == u"nope"_s)
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32000}, {u"message"_s, u"No symbol \"nope\" in current context."_s}}}});
            else
                reply(id, QJsonObject{{u"value"_s, u"%1 = 84"_s.arg(e)}});
        } else if (m == u"watches.get") {
            reply(id, QJsonObject{{u"watches"_s, watches}});
        } else if (m == u"watches.set") {
            watches = p.value(u"watches"_s).toArray();
            reply(id, QJsonObject{{u"watches"_s, watches}});
        } else if (m == u"backtrace.decode") {
            if (p.value(u"project"_s).toString() == u"unbuilt"_s) {
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32000}, {u"message"_s, u"`unbuilt` has no build"_s}}}});
                return;
            }
            QJsonArray found;
            if (p.value(u"text"_s).toString().contains(u"Backtrace:"_s))
                found << QJsonObject{{u"frames"_s, QJsonArray{QJsonObject{{u"addr"_s, u"0x400d1a2c"_s}, {u"func"_s, u"setup()"_s},
                                                                          {u"file"_s, u"/b/blink/esp32/sketch/blink.ino"_s},
                                                                          {u"line"_s, 4}, {u"sketch"_s, true}},
                                                              QJsonObject{{u"addr"_s, u"0x40001234"_s}}}}};
            reply(id, QJsonObject{{u"backtraces"_s, found}});
        } else if (m == u"breakpoints.get") {
            reply(id, QJsonObject{{u"breakpoints"_s, kept}});
        } else if (m == u"breakpoints.set") {
            kept = p.value(u"breakpoints"_s).toArray();
            reply(id, QJsonObject{{u"breakpoints"_s, kept}});
        } else if (m == u"cache.usage") {
            reply(id, QJsonObject{{u"categories"_s, QJsonArray{QJsonObject{{u"kind"_s, u"builds"_s}, {u"path"_s, u"/c/builds"_s}, {u"bytes"_s, cleaned ? 0 : 3000000}},
                                                               QJsonObject{{u"kind"_s, u"arduino"_s}, {u"path"_s, u"/a"_s}, {u"bytes"_s, 900000000}}}},
                                  {u"builds"_s, cleaned ? QJsonArray{} : QJsonArray{QJsonObject{{u"project"_s, u"blink"_s}, {u"bytes"_s, 3000000}}}}});
        } else if (m == u"cache.clean") {
            cleaned = true;
            cleanedWith = p;
            reply(id, QJsonObject{{u"freed"_s, 3000000}});
        } else if (m == u"debug.children") {
            const QString e = p.value(u"expression"_s).toString();
            reply(id, QJsonObject{{u"children"_s, e == u"reading"_s
                       ? QJsonArray{QJsonObject{{u"name"_s, u"celsius"_s}, {u"expression"_s, u"(reading).celsius"_s}, {u"type"_s, u"float"_s}, {u"value"_s, u"21.5"_s}},
                                    QJsonObject{{u"name"_s, u"raw"_s}, {u"expression"_s, u"(reading).raw"_s}, {u"type"_s, u"uint8_t [4]"_s}, {u"children"_s, 4}}}
                       : QJsonArray{}}});
        } else if (m == u"debug.assign") {
            assignedTo << p;
            reply(id, QJsonObject{{u"value"_s, p.value(u"value"_s)}});
        } else if (m == u"debug.watch") {
            reply(id, QJsonObject{{u"watchpoint"_s, QJsonObject{{u"number"_s, ++m_next}, {u"expression"_s, p.value(u"expression"_s)}}}});
        } else if (m == u"debug.memory") {
            reply(id, QJsonObject{{u"address"_s, u"0x3ffb0a10"_s}, {u"bytes"_s, u"48656c6c6f00ff"_s}});
        } else if (m == u"debug.inspect") {
            ++inspections;
            reply(id, QJsonObject{
                          {u"frame"_s, p.value(u"frame"_s).toInt()},
                          {u"locals"_s, QJsonArray{QJsonObject{{u"name"_s, u"pin"_s}, {u"type"_s, u"int"_s}, {u"arg"_s, true}, {u"value"_s, QString::number(inspections + 1)}},
                                                   QJsonObject{{u"name"_s, u"reading"_s}, {u"type"_s, u"Reading"_s}}}},
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
    int m_next = 0; // the last breakpoint number given
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

    void keepsBreakpointsAndShowsLocals()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        const auto count = [&fake](const QString &method) { return fake.methods().count(method); };
        const auto lastOf = [&fake](const QString &method) {
            for (qsizetype i = fake.calls.size() - 1; i >= 0; --i) {
                if (fake.calls.at(i).value(u"method"_s).toString() == method)
                    return fake.calls.at(i).value(u"params"_s).toObject();
            }
            return QJsonObject();
        };

        // Not debugging: the project's kept breakpoints, changed in place.
        panel.setProject(u"blink"_s);
        QTRY_COMPARE(panel.breakpointLocations(), QStringList{u"blink.ino:7"_s});
        QVERIFY(panel.toggleBreakpoint(u"blink.ino:9"_s));
        QCOMPARE(panel.breakpointLocations(), (QStringList{u"blink.ino:7"_s, u"blink.ino:9"_s}));
        QTRY_COMPARE(lastOf(u"breakpoints.set"_s).value(u"breakpoints"_s).toArray().size(), 2);
        QVERIFY(panel.toggleBreakpoint(u"blink.ino:9"_s));
        QTRY_COMPARE(lastOf(u"breakpoints.set"_s).value(u"breakpoints"_s).toArray(), (QJsonArray{u"blink.ino:7"_s}));
        const auto saves = count(u"breakpoints.set"_s);

        // A debug run sets them before the firmware starts, without saving again.
        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_COMPARE(panel.breakpoints()->count(), 1);
        QCOMPARE(lastOf(u"debug.break"_s).value(u"location"_s).toString(), u"blink.ino:7"_s);
        QCOMPARE(count(u"breakpoints.set"_s), saves);
        // One added while paused is set at once and kept.
        QVERIFY(panel.toggleBreakpoint(u"blink.ino:12"_s));
        QTRY_COMPARE(panel.breakpoints()->count(), 2);
        QTRY_COMPARE(lastOf(u"breakpoints.set"_s).value(u"breakpoints"_s).toArray(),
                     (QJsonArray{u"blink.ino:7"_s, u"blink.ino:12"_s}));

        // Stopped: the function's arguments and locals.
        panel.continueButton()->click();
        QTRY_COMPARE(panel.locals()->topLevelItemCount(), 2);
        QCOMPARE(panel.locals()->topLevelItem(0)->text(0), u"pin (argument)"_s);
        QCOMPARE(panel.locals()->topLevelItem(0)->text(1), u"2"_s);
        QCOMPARE(panel.locals()->topLevelItem(1)->text(2), u"Reading"_s);
        QVERIFY(!panel.locals()->topLevelItem(1)->toolTip(1).isEmpty()); // no simple value
        core.stopSim();
        QTRY_VERIFY(!core.isDebugging());
    }

    void watchesExpressionsAndRunsToALine()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        // Not debugging: an expression waits for a stop, and Run to a line says why not.
        panel.addWatch(u"count * 2"_s);
        QCOMPARE(panel.watches()->topLevelItemCount(), 1);
        QVERIFY(!panel.runTo(u"blink.ino:12"_s));
        QVERIFY(panel.statusText().contains(u"Debug"_s));

        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_VERIFY(core.isDebugging());
        // Each stop reads the watched expressions again.
        QTRY_COMPARE(panel.watches()->topLevelItem(0)->text(1), u"count * 2 = 84"_s);
        panel.watchInput()->setText(u"nope"_s);
        QTest::keyClick(panel.watchInput(), Qt::Key_Return);
        QTRY_COMPARE(panel.watches()->topLevelItemCount(), 2);
        QTRY_VERIFY(panel.watches()->topLevelItem(1)->text(1).contains(u"No symbol"_s));
        QVERIFY(panel.watchInput()->text().isEmpty());
        panel.addWatch(u"nope"_s); // once only
        QCOMPARE(panel.watches()->topLevelItemCount(), 2);

        const qsizetype before = fake.methods().count(u"debug.evaluate"_s);
        QVERIFY(panel.runTo(u"blink.ino:12"_s));
        QTRY_VERIFY(panel.statusText().contains(u"line 12"_s));
        // GDB's temporary breakpoint is not called one of the list's.
        QVERIFY(!panel.statusText().contains(u"breakpoint"_s));
        QJsonObject run;
        for (const QJsonObject &c : fake.calls)
            if (c.value(u"method"_s) == u"debug.control"_s)
                run = c.value(u"params"_s).toObject();
        QCOMPARE(run.value(u"action"_s).toString(), u"runTo"_s);
        QCOMPARE(run.value(u"location"_s).toString(), u"blink.ino:12"_s);
        QTRY_COMPARE(fake.methods().count(u"debug.evaluate"_s), before + 2);
        core.stopSim();
    }

    void keepsWatchesAndConditions()
    {
        FakeCore fake(path());
        fake.watches = QJsonArray{u"count"_s};
        fake.kept = QJsonArray{u"blink.ino:7 if i == 3"_s, u"setup"_s};
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        panel.setProject(u"blink"_s);
        // The project's kept watches come back.
        QTRY_COMPARE(panel.watchExpressions(), QStringList{u"count"_s});
        panel.addWatch(u"buf[3]"_s);
        QTRY_COMPARE(fake.watches, (QJsonArray{u"count"_s, u"buf[3]"_s}));
        panel.removeWatch(u"count"_s);
        QTRY_COMPARE(fake.watches, QJsonArray{u"buf[3]"_s});

        // Not debugging: a condition changes the kept breakpoint.
        QTRY_COMPARE(panel.breakpointLocations().size(), 2);
        QCOMPARE(DebugPanel::keptLocation(u"blink.ino:7 if i == 3"_s), u"blink.ino:7"_s);
        QCOMPARE(DebugPanel::keptCondition(u"blink.ino:7 if i == 3"_s), u"i == 3"_s);
        QVERIFY(panel.setCondition(u"setup"_s, u"ready"_s));
        QTRY_COMPARE(fake.kept, (QJsonArray{u"blink.ino:7 if i == 3"_s, u"setup if ready"_s}));
        QVERIFY(panel.setCondition(u"blink.ino:7"_s, QString()));
        QTRY_COMPARE(fake.kept, (QJsonArray{u"blink.ino:7"_s, u"setup if ready"_s}));
        // The gutter's toggle finds a conditional one by its line.
        QVERIFY(panel.toggleBreakpoint(u"setup"_s));
        QTRY_COMPARE(fake.kept, QJsonArray{u"blink.ino:7"_s});
        QVERIFY(panel.setCondition(u"blink.ino:9"_s, u"x > 1"_s)); // a new one
        QTRY_COMPARE(fake.kept, (QJsonArray{u"blink.ino:7"_s, u"blink.ino:9 if x > 1"_s}));

        // A debug run sets them with their conditions.
        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_VERIFY(core.isDebugging());
        QTRY_COMPARE(panel.breakpoints()->count(), 2);
        QJsonObject conditional;
        for (const QJsonObject &c : fake.calls)
            if (c.value(u"method"_s) == u"debug.break"_s && c.value(u"params"_s).toObject().contains(u"condition"_s))
                conditional = c.value(u"params"_s).toObject();
        QCOMPARE(conditional.value(u"location"_s).toString(), u"blink.ino:9"_s);
        QCOMPARE(conditional.value(u"condition"_s).toString(), u"x > 1"_s);
        QVERIFY(panel.breakpoints()->item(1)->text().contains(u"if x > 1"_s));
        // Paused: a new condition is GDB's breakpoint again.
        const qsizetype deletes = fake.methods().count(u"debug.delete"_s);
        QVERIFY(panel.setCondition(u"blink.ino:7"_s, u"y"_s));
        QTRY_COMPARE(fake.methods().count(u"debug.delete"_s), deletes + 1);
        QTRY_VERIFY(fake.kept.contains(QJsonValue(u"blink.ino:7 if y"_s)));
        QVERIFY(!fake.kept.contains(QJsonValue(u"blink.ino:7"_s)));
        core.stopSim();
    }

    void decodesAPastedBacktrace()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        BacktraceDialog dialog(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QVERIFY(!dialog.decodeButton()->isEnabled());
        dialog.setBuild(u"blink"_s, u"esp32"_s, u"Blink"_s);
        dialog.input()->setPlainText(u"Guru Meditation Error\nBacktrace: 0x400d1a2c:0x3ffb0000"_s);
        QVERIFY(dialog.decodeButton()->isEnabled());
        dialog.decodeButton()->click();
        QTRY_COMPARE(dialog.frames()->topLevelItemCount(), 2);
        QCOMPARE(dialog.frames()->topLevelItem(0)->text(1), u"blink.ino, line 4"_s);
        QVERIFY(dialog.frames()->topLevelItem(0)->font(0).bold());
        QSignalSpy at(&dialog, &BacktraceDialog::locationActivated);
        emit dialog.frames()->itemActivated(dialog.frames()->topLevelItem(0), 0);
        QCOMPARE(at.size(), 1);
        QCOMPARE(at.at(0).at(0).toJsonObject().value(u"line"_s).toInt(), 4);
        emit dialog.frames()->itemActivated(dialog.frames()->topLevelItem(1), 0); // no line: nothing
        QCOMPARE(at.size(), 1);
        // No backtrace in the text, and a project with no build.
        dialog.input()->setPlainText(u"hello"_s);
        dialog.decodeButton()->click();
        QTRY_VERIFY(dialog.statusText().startsWith(u"No backtrace"_s));
        QVERIFY(!dialog.frames()->isVisibleTo(&dialog));
        dialog.setBuild(u"unbuilt"_s, u"esp32"_s, u"Unbuilt"_s);
        dialog.decodeButton()->click();
        QTRY_VERIFY(dialog.statusText().contains(u"has no build"_s));
    }

    void looksInFramesWatchesChangesAndReadsMemory()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_VERIFY(core.isDebugging());
        QTRY_COMPARE(panel.stack()->topLevelItemCount(), 2);
        QTRY_COMPARE(panel.locals()->topLevelItemCount(), 2);

        // A caller's frame: inspected there, and its line asked to be marked.
        QSignalSpy frames(&panel, &DebugPanel::frameSelected);
        panel.selectFrame(1);
        QTRY_COMPARE(panel.selectedFrame(), 1);
        QCOMPARE(frames.last().at(0).toJsonObject().value(u"func"_s).toString(), u"loopTask"_s);
        QJsonObject asked;
        for (const QJsonObject &c : fake.calls)
            if (c.value(u"method"_s) == u"debug.inspect"_s)
                asked = c.value(u"params"_s).toObject();
        QCOMPARE(asked.value(u"frame"_s).toInt(), 1);
        panel.selectFrame(0);
        QTRY_COMPARE(panel.selectedFrame(), 0);

        // A double-click on a frame opens its source, where it has one.
        QSignalSpy source(&panel, &DebugPanel::sourceRequested);
        emit panel.stack()->itemActivated(panel.stack()->topLevelItem(1), 0); // loopTask: no file
        QCOMPARE(source.size(), 0);
        emit panel.stack()->itemActivated(panel.stack()->topLevelItem(0), 0);
        QCOMPARE(source.size(), 1);
        QCOMPARE(source.at(0).at(0).toJsonObject().value(u"file"_s).toString(), u"blink.ino"_s);
        QCOMPARE(source.at(0).at(0).toJsonObject().value(u"line"_s).toInt(), 4);
        QTRY_COMPARE(panel.selectedFrame(), 0);

        // A watchpoint from a watched expression, and its stop.
        panel.addWatch(u"count"_s);
        QVERIFY(panel.watchChanges(u"count"_s));
        // No breakpoints are kept here: the watchpoint is the list's only one.
        QTRY_COMPARE(panel.breakpoints()->count(), 1);
        QVERIFY(panel.breakpoints()->item(0)->text().contains(u"when count changes"_s));
        const QString pinBefore = panel.locals()->topLevelItem(0)->text(1);
        fake.stopAtWatch();
        QTRY_VERIFY(panel.statusText().contains(u"count changed from 1 to 2"_s));
        // What changed since the last stop is bold.
        QTRY_VERIFY(panel.locals()->topLevelItem(0) && panel.locals()->topLevelItem(0)->text(1) != pinBefore);
        QVERIFY(panel.locals()->topLevelItem(0)->font(1).bold());
        QVERIFY(!panel.locals()->topLevelItem(1)->font(1).bold()); // the struct: unchanged
        QVERIFY(!panel.registers()->topLevelItem(0)->font(1).bold());

        // Memory, as a hex dump, read again at each stop.
        panel.memoryInput()->setText(u"&greeting"_s);
        QTest::keyClick(panel.memoryInput(), Qt::Key_Return);
        QTRY_VERIFY(panel.memoryText().startsWith(u"3ffb0a10  48 65 6c 6c 6f 00 ff"_s));
        QVERIFY(panel.memoryText().endsWith(u"Hello.."_s));
        core.stopSim();
    }

    void countsHitsPassesAndShowsHex()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        panel.hexBox()->setChecked(false);
        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_VERIFY(core.isDebugging());
        core.debugBreak(u"setup"_s);
        QTRY_COMPARE(panel.breakpoints()->count(), 1);
        panel.continueButton()->click();
        QTRY_VERIFY(panel.statusText().startsWith(u"Stopped at breakpoint 1"_s));
        QVERIFY(!panel.statusText().contains(u"reached"_s)); // the first time
        panel.continueButton()->click();
        QTRY_VERIFY2(panel.statusText().contains(u"it has been reached 2 times"_s), qPrintable(panel.statusText()));
        QCOMPARE(panel.breakpoints()->item(0)->toolTip(), u"Reached 2 times in this run"_s);

        // Pass it the next 499 times.
        QVERIFY(panel.ignoreBreakpoint(1, 499));
        QTRY_VERIFY(fake.methods().contains(u"debug.ignore"_s));
        QJsonObject asked;
        for (const QJsonObject &c : fake.calls)
            if (c.value(u"method"_s) == u"debug.ignore"_s)
                asked = c.value(u"params"_s).toObject();
        QCOMPARE(asked.value(u"breakpoint"_s).toInt(), 1);
        QCOMPARE(asked.value(u"count"_s).toInt(), 499);

        // Hex: whole numbers only, and back.
        QTRY_VERIFY(panel.locals()->topLevelItemCount() == 2);
        const QString pin = panel.locals()->topLevelItem(0)->text(1);
        panel.hexBox()->setChecked(true);
        QCOMPARE(panel.locals()->topLevelItem(0)->text(1), u"0x%1"_s.arg(QString::number(pin.toInt(), 16)));
        QCOMPARE(panel.locals()->topLevelItem(1)->text(1), u"…"_s); // a struct: as it was
        panel.hexBox()->setChecked(false);
        QCOMPARE(panel.locals()->topLevelItem(0)->text(1), pin);
        QCOMPARE(DebugPanel::shownValue(u"-26"_s, true), u"-0x1a"_s);
        QCOMPARE(DebugPanel::shownValue(u"3.5"_s, true), u"3.5"_s);
        QCOMPARE(DebugPanel::shownValue(u"65 'A'"_s, true), u"65 'A'"_s);
        core.stopSim();
    }

    void opensStructuresAndSetsValues()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DebugPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"blink"_s, u"esp32"_s, true);
        QTRY_VERIFY(core.isDebugging());
        QTRY_COMPARE(panel.locals()->topLevelItemCount(), 2);
        // The struct can be opened: its members come from the core.
        QTreeWidgetItem *reading = panel.locals()->topLevelItem(1);
        QCOMPARE(reading->childIndicatorPolicy(), QTreeWidgetItem::ShowIndicator);
        panel.expandItem(reading);
        QTRY_COMPARE(reading->childCount(), 2);
        QCOMPARE(reading->child(0)->text(0), u"celsius"_s);
        QCOMPARE(reading->child(0)->text(1), u"21.5"_s);
        QCOMPARE(reading->child(1)->childIndicatorPolicy(), QTreeWidgetItem::ShowIndicator); // an array, in turn
        // Setting a value: the core sets it, and the variables are read again.
        const qsizetype inspections = fake.methods().count(u"debug.inspect"_s);
        QVERIFY(panel.assign(u"(reading).celsius"_s, u"30"_s));
        QTRY_COMPARE(fake.assignedTo.size(), 1);
        QCOMPARE(fake.assignedTo.at(0).value(u"expression"_s).toString(), u"(reading).celsius"_s);
        QCOMPARE(fake.assignedTo.at(0).value(u"value"_s).toString(), u"30"_s);
        QTRY_VERIFY(panel.statusText().contains(u"(reading).celsius is now 30"_s));
        QTRY_COMPARE(fake.methods().count(u"debug.inspect"_s), inspections + 1);
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());
        QVERIFY(!panel.assign(u"x"_s, u"1"_s)); // not paused
    }

    void saysTheDiskSpaceAndRemovesBuilds()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DiskSpaceDialog dialog(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        dialog.refresh();
        QTRY_COMPARE(dialog.tree()->topLevelItemCount(), 2);
        QTreeWidgetItem *builds = dialog.tree()->topLevelItem(0);
        QCOMPARE(builds->text(0), u"Builds"_s);
        QCOMPARE(builds->childCount(), 1);
        QCOMPARE(builds->child(0)->text(0), u"blink"_s);
        QVERIFY(!dialog.removeButton()->isEnabled());
        dialog.tree()->setCurrentItem(builds->child(0));
        QVERIFY(dialog.removeButton()->isEnabled());
        dialog.removeButton()->click();
        QTRY_VERIFY(fake.cleaned);
        QCOMPARE(fake.cleanedWith.value(u"project"_s).toString(), u"blink"_s);
        QTRY_VERIFY(dialog.statusText().startsWith(u"Freed"_s));
        QTRY_COMPARE(dialog.tree()->topLevelItem(0)->childCount(), 0); // read again
        QVERIFY(!dialog.removeAllButton()->isEnabled());
    }

    void dumpsMemoryAsHex()
    {
        const QByteArray bytes = QByteArray("0123456789abcdefXY");
        const QString dump = DebugPanel::hexDump(0x3ffb0000, bytes);
        const QStringList rows = dump.split(u'\n');
        QCOMPARE(rows.size(), 2);
        QCOMPARE(rows.at(0), u"3ffb0000  30 31 32 33 34 35 36 37  38 39 61 62 63 64 65 66  0123456789abcdef"_s);
        QVERIFY(rows.at(1).startsWith(u"3ffb0010  58 59 "_s));
        QVERIFY(rows.at(1).endsWith(u"XY"_s));
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
        QVERIFY(!build.debugAction()->isEnabled()); // nothing built yet
        // A plain run does not ask for the debugger.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(!core.isDebugging());
        QCOMPARE(fake.calls.first().value(u"params"_s).toObject().value(u"debug"_s), QJsonValue(false));
    }
};

QTEST_MAIN(TestDebugger)
#include "tst_debugger.moc"
