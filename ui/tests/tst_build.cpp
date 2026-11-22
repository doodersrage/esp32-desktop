// The build flow against a fake core: consent refusal, retry with the token, early
// events, and MainWindow asking the user (answered here without a dialog).

#include "BuildPanel.h"
#include "CoreConnection.h"
#include "MainWindow.h"
#include "MessageBar.h"
#include "ProjectView.h"

#include <QComboBox>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QStandardPaths>
#include <QCheckBox>
#include <QPushButton>
#include <QAction>
#include <QKeySequence>
#include <QLineEdit>
#include <QTest>

using namespace Qt::StringLiterals;

// A fake core that knows core.hello and build.start. build.start without the right
// token is refused with -32002; with it, the build's events are sent *before* the
// reply, which docs/rpc.md allows, to check the UI waits for its job id.
class FakeCore : public QObject {
    Q_OBJECT
public:
    QStringList buildRequests; // the consent value of each build.start, or "-"
    QStringList statusRequests; // each build.status, as project/board

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
                                               {u"protocol"_s, u"0.42.0"_s}}}});
        } else if (method == u"build.status") {
            statusRequests << params.value(u"project"_s).toString() + u"/"_s + params.value(u"board"_s).toString();
            send(s, {{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                     {u"result"_s, QJsonObject{{u"built"_s, true}, {u"builtAt"_s, 1795000000}, {u"current"_s, true}}}});
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
            if (fail)
                send(s, event(u"build.finished"_s,
                              {{u"ok"_s, false}, {u"error"_s, u"compile failed (exit status 1)"_s},
                               {u"firstError"_s, QJsonObject{{u"file"_s, u"blink.ino"_s}, {u"line"_s, 12},
                                                             {u"column"_s, 3},
                                                             {u"message"_s, u"'ledd' was not declared"_s}}}}));
            else
                send(s, event(u"build.finished"_s, {{u"ok"_s, true}, {u"image"_s, u"/tmp/blink.bin"_s}}));
            send(s, {{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                     {u"result"_s, QJsonObject{{u"job"_s, 9}, {u"plan"_s, plan}}}});
        }
    }

public:
    bool fail = false; // the build fails, with a compiler error

private:
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
    // A project opened on the board the picker already shows still asks whether it is
    // built, so Run uses the build from before instead of asking to build it again.
    void asksWhetherAShownProjectIsBuilt()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        ProjectView view(&core);
        QSignalSpy current(&view, &ProjectView::buildCurrent);
        QFile f(QStringLiteral(ESP32BENCH_SOURCE_DIR "/../schema/rpc/library.get.json"));
        QVERIFY(f.open(QIODevice::ReadOnly));
        const QJsonObject result = QJsonDocument::fromJson(f.readAll())
                                       .object().value(u"examples"_s).toArray().at(0).toObject()
                                       .value(u"result"_s).toObject();
        view.showResult(result);
        const QString id = result.value(u"project"_s).toObject().value(u"id"_s).toString();
        QTRY_COMPARE(fake.statusRequests.size(), 1);
        QVERIFY(fake.statusRequests.at(0).startsWith(id + u"/"_s));
        QTRY_VERIFY(!current.isEmpty() && current.last().at(2).toBool());
        QVERIFY(view.buildStateText().contains(u"ready to run"_s));
        QVERIFY(fake.buildRequests.isEmpty());
    }

    // Keep QSettings away from the real configuration.
    void initTestCase() { QStandardPaths::setTestModeEnabled(true); }

    void portsAreNamedAndANewBoardIsPicked()
    {
        CoreConnection core;
        BuildPanel panel(&core);
        QComboBox *ports = panel.portPicker();
        const auto list = [](const char *json) { return QJsonDocument::fromJson(json).array(); };
        emit core.devicesListed(list(R"([
          {"path": "/dev/ttyACM0", "bridge": "usb-serial-jtag", "chips": ["esp32s3"]},
          {"path": "/dev/ttyUSB0", "bridge": "cp210x", "chips": []},
          {"path": "/dev/ttyS0", "chips": []}
        ])"));
        // What is on the port first, then its short name; the path is the data.
        QCOMPARE(ports->itemText(0), u"ESP32-S3 – ttyACM0"_s);
        QCOMPARE(ports->itemText(1), u"CP210x USB to UART – ttyUSB0"_s);
        QCOMPARE(ports->itemText(2), u"ttyS0"_s);
        QCOMPARE(ports->itemData(1).toString(), u"/dev/ttyUSB0"_s);
        // The first list picks the first port, and the user's choice stays...
        QCOMPARE(ports->currentData().toString(), u"/dev/ttyACM0"_s);
        ports->setCurrentIndex(2);
        emit core.devicesListed(list(R"([
          {"path": "/dev/ttyACM0", "chips": ["esp32s3"]}, {"path": "/dev/ttyUSB0", "chips": []},
          {"path": "/dev/ttyS0", "chips": []}])"));
        QCOMPARE(ports->currentData().toString(), u"/dev/ttyS0"_s);
        // ...until a board is plugged in: that is the one to flash.
        emit core.devicesListed(list(R"([
          {"path": "/dev/ttyACM0", "chips": ["esp32s3"]}, {"path": "/dev/ttyUSB0", "chips": []},
          {"path": "/dev/ttyS0", "chips": []}, {"path": "/dev/ttyACM1", "chips": ["esp32c3"]}])"));
        QCOMPARE(ports->currentData().toString(), u"/dev/ttyACM1"_s);
    }

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

    void aFailedBuildSaysItsFirstError()
    {
        FakeCore fake(path());
        fake.fail = true;
        CoreConnection core;
        core.setSocketPath(path());
        AnsweringWindow window(&core);
        window.answer = true;
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startBuild(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(window.messageBar()->text().contains(u"blink.ino, line 12"_s));
        QVERIFY(window.messageBar()->text().contains(u"'ledd' was not declared"_s));
    }

    void countsAndFiltersProblems()
    {
        CoreConnection core;
        BuildPanel panel(&core);
        emit core.buildLog(u"stderr"_s, u"a.ino:1:1: warning: unused"_s, u"warning"_s);
        emit core.buildLog(u"stdout"_s, u"Compiling…"_s);
        emit core.buildLog(u"stderr"_s, u"a.ino:2:1: error: oops"_s, u"error"_s);
        QVERIFY(panel.problemText().contains(u"Errors: 1"_s));
        QVERIFY(panel.problemText().contains(u"Warnings: 1"_s));
        panel.problemsOnlyBox()->setChecked(true);
        QCOMPARE(panel.visibleLogLines(), (QStringList{u"a.ino:1:1: warning: unused"_s, u"a.ino:2:1: error: oops"_s}));
        panel.problemsOnlyBox()->setChecked(false);
        QCOMPARE(panel.visibleLogLines().size(), 3);
    }

    void locatedLinesCopyAndEscape()
    {
        CoreConnection core;
        BuildPanel panel(&core);
        QVERIFY(panel.copyProblemsButton()->isHidden());
        QVERIFY(!panel.cancelRunning()); // nothing running
        emit core.buildStarted(QJsonObject{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s}});
        const QJsonObject at{{u"file"_s, u"blink.ino"_s}, {u"line"_s, 12}, {u"column"_s, 3}};
        emit core.buildLog(u"stdout"_s, u"Compiling…"_s);
        emit core.buildLog(u"stderr"_s, u"/tmp/b/blink.ino:12:3: error: oops"_s, u"error"_s, at);
        emit core.buildLog(u"stderr"_s, u"collect2: error: ld returned 1 exit status"_s, u"error"_s);
        // A double-click (or Enter) on a line that names a place asks to show it.
        QSignalSpy shown(&panel, &BuildPanel::locationActivated);
        QVERIFY(!panel.activateLine(0));
        QVERIFY(panel.activateLine(1));
        QCOMPARE(shown.size(), 1);
        QCOMPARE(shown.at(0).at(0).toJsonObject(), at);
        QVERIFY(!panel.activateLine(2));
        // Find in the log, round from the end.
        QVERIFY(panel.findText(u"oops"_s));
        QVERIFY(panel.findText(u"Compiling"_s, true));
        QVERIFY(!panel.findText(u"no such words"_s));
        QCOMPARE(panel.findField()->toolTip(), u"Not found"_s);
        // F8 and Shift+F8: the next and previous located problem, round from the end.
        QVERIFY(panel.nextProblemAction()->isEnabled());
        QCOMPARE(panel.nextProblemAction()->shortcut(), QKeySequence(Qt::Key_F8));
        const QJsonObject later{{u"file"_s, u"blink.ino"_s}, {u"line"_s, 20}};
        emit core.buildLog(u"stderr"_s, u"/tmp/b/blink.ino:20: warning: unused"_s, u"warning"_s, later);
        shown.clear();
        QVERIFY(panel.goToProblem(true));
        QVERIFY(panel.goToProblem(true));
        QVERIFY(panel.goToProblem(true)); // round to the first again
        QCOMPARE(shown.size(), 3);
        QCOMPARE(shown.at(0).at(0).toJsonObject(), at);
        QCOMPARE(shown.at(1).at(0).toJsonObject(), later);
        QCOMPARE(shown.at(2).at(0).toJsonObject(), at);
        QVERIFY(panel.goToProblem(false));
        QCOMPARE(shown.last().at(0).toJsonObject(), later);
        // A new build has none yet.
        emit core.buildStarted(QJsonObject{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s}});
        QVERIFY(!panel.nextProblemAction()->isEnabled());
        QVERIFY(!panel.goToProblem(true));
        emit core.buildLog(u"stderr"_s, u"/tmp/b/blink.ino:12:3: error: oops"_s, u"error"_s, at);
        emit core.buildLog(u"stderr"_s, u"collect2: error: ld returned 1 exit status"_s, u"error"_s);
        // Copy for a forum post: the problems as a code block, with the project and board.
        QVERIFY2(panel.forumText().contains(u"building blink for esp32\n```text\n/tmp/b/blink.ino:12:3: error: oops\n"_s),
                 qPrintable(panel.forumText()));
        QVERIFY(!panel.forumText().contains(u"Compiling"_s));
        // Copy problems: only the error and warning lines.
        QVERIFY(!panel.copyProblemsButton()->isHidden());
        QCOMPARE(panel.problemLines(),
                 u"/tmp/b/blink.ino:12:3: error: oops\ncollect2: error: ld returned 1 exit status"_s);
        // Esc in the dock stops the running build, once.
        QVERIFY(panel.cancelRunning());
        emit core.buildFinished(false, u"cancelled"_s, {});
        QVERIFY(!panel.cancelRunning());
    }

    void aPlaceOutsideTheProjectIsSaidSo()
    {
        CoreConnection core;
        MainWindow window(&core);
        emit window.buildPanel()->locationActivated(
            QJsonObject{{u"file"_s, u"Wire.cpp"_s}, {u"line"_s, 40}});
        QVERIFY2(window.messageBar()->text().contains(u"Line 40 of Wire.cpp"_s), qPrintable(window.messageBar()->text()));
    }

    void saysTheSizeAndFlashSpeed()
    {
        CoreConnection core;
        BuildPanel panel(&core);
        emit core.buildStarted(QJsonObject{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s}});
        emit core.buildSize(QJsonObject{{u"flash"_s, 1'200'000}, {u"flashMax"_s, 1'310'720},
                                        {u"ram"_s, 20'000}, {u"ramMax"_s, 327'680}});
        emit core.buildFinished(true, {}, u"/tmp/x/blink.ino.merged.bin"_s);
        QVERIFY(panel.statusText().contains(u"92%"_s));
        QVERIFY(panel.statusText().contains(u"6%"_s));
        QVERIFY(panel.statusText().contains(u"nearly full"_s));
        // How long it took.
        emit core.buildStarted(QJsonObject{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s}});
        emit core.buildTime(75.4);
        emit core.buildFinished(true, {}, u"/tmp/x/blink.ino.merged.bin"_s);
        QVERIFY2(panel.statusText().contains(u"It took 1 min 15 s."_s), qPrintable(panel.statusText()));
        emit core.buildFinished(false, u"x"_s, {});
        QVERIFY(!panel.statusText().contains(u"It took"_s)); // not the last build's time
        // Flash progress: the percent, then the speed and time left.
        QVERIFY(panel.flashProgressText(0, 1000).startsWith(u"Writing 0%"_s));
        QTest::qWait(600);
        const QString text = panel.flashProgressText(500, 1000);
        QVERIFY2(text.contains(u"/s"_s) && text.contains(u"left"_s), qPrintable(text));
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
                QVERIFY(window.buildPanel()->statusText().contains(u"blink.bin"_s));
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
