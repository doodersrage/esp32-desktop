// SimulatorPanel and CoreConnection's sim calls against a fake core.

#include "CoreConnection.h"
#include "PlotPanel.h"
#include "SimulatorPanel.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QLineEdit>
#include <QLocale>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QCheckBox>
#include <QSettings>
#include <QStandardPaths>
#include <QSignalSpy>
#include <QTest>

#include <cmath>

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
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.33.0"_s}});
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
            if (p.value(u"project"_s).toString() == u"oled"_s) {
                // An expectation that is met in part, then times out.
                reply(id, r);
                const auto expect = [this](const QJsonArray &matched, const QJsonArray &missing, bool timedOut) {
                    send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.expect"_s},
                          {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"matched"_s, matched},
                                                    {u"missing"_s, missing}, {u"needs"_s, u"parts"_s},
                                                    {u"seconds"_s, 10}, {u"timedOut"_s, timedOut}}}});
                };
                expect({}, {u"Display ready"_s, u"temp -?\\d+\\.\\d+ C"_s}, false);
                expect({u"Display ready"_s}, {u"temp -?\\d+\\.\\d+ C"_s}, false);
                expect({u"Display ready"_s}, {u"temp -?\\d+\\.\\d+ C"_s}, true);
                return;
            }
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
            if (p.value(u"project"_s).toString() == u"crashy"_s) {
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.output"_s},
                      {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"data"_s, QString::fromLatin1(QByteArray("Guru\n").toBase64())},
                                                {u"text"_s, u"Guru\n"_s},
                                                {u"explain"_s, QJsonArray{QJsonObject{{u"key"_s, u"panic-load"_s},
                                                                                      {u"text"_s, u"Null pointer."_s},
                                                                                      {u"level"_s, u"problem"_s}}}}}}});
            }
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

    void showsTheSimulatedTime()
    {
        CoreConnection core;
        SimulatorPanel panel(&core);
        QVERIFY(panel.clockText().isEmpty());
        emit core.simClock(12'449'000'000);
        QCOMPARE(panel.clockText(), u"%1 s simulated"_s.arg(QLocale().toString(12.4, 'f', 1)));
        emit core.simClock(754'000'000'000);
        QCOMPARE(panel.clockText(), u"12 min 34 s simulated"_s);
        // A new run starts from nothing.
        emit core.simStarted(u"blink"_s, u"esp32"_s, u"esp32"_s);
        QVERIFY(panel.clockText().isEmpty());
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

    void saysWhetherTheOutputIsAsExpected()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SimulatorPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"oled"_s, u"esp32"_s);
        // Half seen, then the time ran out: what is missing, readably, and why it may be.
        QTRY_VERIFY(panel.expectText().startsWith(u"Not seen yet"_s));
        QVERIFY(panel.expectText().contains(u"temp … C"_s));
        QVERIFY(panel.expectText().contains(u"Circuit dock"_s));
        QCOMPARE(SimulatorPanel::readablePattern(u"tick \\d+ ms"_s), u"tick … ms"_s);
        QCOMPARE(SimulatorPanel::readablePattern(u"Sleeping\\. Press"_s), u"Sleeping. Press"_s);
        QCOMPARE(SimulatorPanel::readablePattern(u"ota ready: .+ at"_s), u"ota ready: … at"_s);
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());
        // A new run starts without the last one's verdict.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(panel.expectText().isEmpty());
    }

    void passesOnExplanations()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        QSignalSpy explained(&core, &CoreConnection::outputExplained);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"crashy"_s, u"esp32"_s);
        QTRY_COMPARE(explained.size(), 1);
        QCOMPARE(explained.at(0).at(0).toString(), u"sim"_s);
        QCOMPARE(explained.at(0).at(1).toJsonArray().at(0).toObject().value(u"key"_s).toString(), u"panic-load"_s);
    }

    void plotsRowsOfNumbers()
    {
        CoreConnection core;
        PlotPanel panel(&core);
        QSignalSpy first(&panel, &PlotPanel::firstRows);
        // One row in a list of rows. Not QJsonArray{row(...)}: MSVC reads braces
        // around a single QJsonArray as a copy of it, not a list holding it.
        const auto oneRow = [](const QJsonArray &r) {
            QJsonArray rows;
            rows.append(r);
            return rows;
        };
        const auto row = [](double t, double h) {
            return QJsonArray{QJsonObject{{u"name"_s, u"temp"_s}, {u"value"_s, t}},
                              QJsonObject{{u"name"_s, u"hum"_s}, {u"value"_s, h}}};
        };
        emit core.plotRows(QJsonArray{row(21.5, 45), row(21.6, 44)});
        QCOMPARE(first.size(), 1); // shows the dock once
        QCOMPARE(panel.canvas()->seriesNames(), (QStringList{u"temp"_s, u"hum"_s}));
        QCOMPARE(panel.canvas()->values(u"temp"_s), (QList<double>{21.5, 21.6}));
        // Every row received is kept for Save CSV…, as the core sent it; counted step by step.
        qsizetype kept = 2;
        QCOMPARE(panel.rows().size(), kept);
        // Paused: nothing added; then cleared.
        panel.findChild<QCheckBox *>()->setChecked(true);
        emit core.plotRows(oneRow(row(30, 30)));
        QCOMPARE(panel.canvas()->values(u"temp"_s).size(), 2);
        QCOMPARE(panel.rows().size(), kept);
        panel.findChild<QCheckBox *>()->setChecked(false);
        // Only the latest rows are kept.
        QJsonArray many;
        for (int i = 0; i < PlotCanvas::Kept + 20; ++i)
            many.append(row(i, i));
        emit core.plotRows(many);
        QCOMPARE(panel.canvas()->values(u"hum"_s).size(), PlotCanvas::Kept);
        QCOMPARE(panel.canvas()->values(u"hum"_s).last(), double(PlotCanvas::Kept + 19));
        kept += PlotCanvas::Kept + 20;
        QCOMPARE(panel.rows().size(), kept);
        panel.resize(600, 300);
        QVERIFY(!panel.grab().isNull()); // draws
        if (const QString shot = qEnvironmentVariable("ESP32BENCH_SCREENSHOT"); !shot.isEmpty()) {
            panel.canvas()->clear();
            QJsonArray wave;
            for (int i = 0; i < 200; ++i)
                wave.append(row(21 + std::sin(i / 15.0), 45 + 3 * std::cos(i / 25.0)));
            emit core.plotRows(wave);
            kept += 200;
            QVERIFY(panel.grab().save(shot));
        }
        panel.canvas()->clear();
        QVERIFY(panel.canvas()->seriesNames().isEmpty());
        // A click on a legend entry hides that line, and again shows it.
        emit core.plotRows(oneRow(row(1, 2)));
        kept += 1;
        QCOMPARE(panel.rows().size(), kept);
        panel.resize(600, 300);
        panel.grab(); // paints, which places the legend
        QVERIFY(!panel.canvas()->legend().isEmpty());
        const auto [rect, name] = panel.canvas()->legend().first();
        QTest::mouseClick(panel.canvas(), Qt::LeftButton, {}, rect.center().toPoint());
        QVERIFY(panel.canvas()->isHidden(name));
        QTest::mouseClick(panel.canvas(), Qt::LeftButton, {}, rect.center().toPoint());
        QVERIFY(!panel.canvas()->isHidden(name));
        QCOMPARE(panel.rows().size(), kept);
        QCOMPARE(panel.rows().at(0).toArray(), row(21.5, 45));
        QVERIFY(panel.saveButton()->isEnabled());
        // Save picture…: the plot as drawn.
        QVERIFY(panel.savePictureButton()->isEnabled());
        const QImage picture = panel.picture();
        QVERIFY(!picture.isNull());
        QVERIFY(picture.width() <= 4096 && picture.height() <= 4096);
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
