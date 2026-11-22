// SerialMonitor and CoreConnection's serial calls against a fake core.

#include "ConsoleView.h"
#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "MainWindow.h"
#include "MessageBar.h"
#include "Preferences.h"
#include "ProblemsPanel.h"
#include "CommandPalette.h"
#include "SerialMonitor.h"

#include <QCheckBox>
#include <QScrollBar>
#include <QAction>
#include <QComboBox>
#include <QDockWidget>
#include <QTreeWidget>
#include <QSignalSpy>
#include <QSpinBox>
#include <QTextBlock>
#include <QTextCursor>
#include <QToolBar>
#include <QDir>
#include <QKeySequence>
#include <QMimeData>
#include <QSettings>
#include <QToolButton>
#include <QUrl>
#include <QUrlQuery>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLineEdit>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QStandardPaths>
#include <QRegularExpression>
#include <QMenuBar>
#include <QLocale>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

// Knows core.hello, device.watch, serial.open (sends output *before* its reply),
// serial.write (records the bytes) and core.unsubscribe. pushClosed() unplugs.
class FakeCore : public QObject {
    Q_OBJECT
public:
    QList<QByteArray> written;
    QStringList opened;
    bool reconnect = false;

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

    void pushData(const QByteArray &bytes)
    {
        send(event(u"serial.data"_s, {{u"data"_s, QString::fromLatin1(bytes.toBase64())},
                                      {u"text"_s, QString::fromUtf8(bytes)}}));
    }
    void pushClosed() { send(event(u"serial.closed"_s, {{u"reason"_s, u"the port was closed"_s}})); }
    void pushLost() { send(event(u"serial.lost"_s, {{u"reason"_s, u"the port was closed"_s}})); }
    void pushReconnected() { send(event(u"serial.reconnected"_s, {})); }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    static QJsonObject event(const QString &method, QJsonObject params)
    {
        params[u"subscription"_s] = 4;
        return {{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, method}, {u"params"_s, params}};
    }
    void reply(const QJsonValue &id, const QJsonValue &result)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, result}});
    }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s},
                                  {u"protocol"_s, u"0.46.0"_s}});
        } else if (m == u"device.watch") {
            reply(id, QJsonObject{{u"subscription"_s, 1},
                                  {u"ports"_s, QJsonArray{QJsonObject{{u"path"_s, u"/dev/ttyUSB0"_s},
                                                                      {u"bridge"_s, u"usb-serial-jtag"_s}}}}});
        } else if (m == u"serial.open") {
            opened << p.value(u"path"_s).toString() + u"@"_s + QString::number(p.value(u"baud"_s).toInt());
            reconnect = p.value(u"reconnect"_s).toBool();
            // Output that beats the reply; the UI must still show it, in order.
            // "é" is split across two chunks to check decoding keeps state.
            pushData("ets Jul 29 2019\r\ncaf\xc3");
            pushData("\xa9 ok\n");
            reply(id, QJsonObject{{u"subscription"_s, 4}});
        } else if (m == u"serial.write") {
            written << QByteArray::fromBase64(p.value(u"data"_s).toString().toLatin1());
            reply(id, QJsonObject{});
        } else if (m == u"core.unsubscribe") {
            reply(id, QJsonObject{});
        } else if (m == u"serial.record") {
            const QString to = p.value(u"path"_s).toString();
            recorded << to;
            reply(id, QJsonObject{{u"recording"_s, !to.isEmpty()}, {u"path"_s, to.isEmpty() ? recording : to},
                                  {u"bytes"_s, to.isEmpty() ? 42 : 0}});
            recording = to;
        } else if (m == u"serial.findBaud") {
            reply(id, QJsonObject{{u"baud"_s, 9600}, {u"tried"_s, QJsonArray{QJsonObject{{u"baud"_s, 115200}, {u"chars"_s, 90}, {u"readable"_s, 0.3}},
                                                                              QJsonObject{{u"baud"_s, 9600}, {u"chars"_s, 60}, {u"readable"_s, 0.99}}}}});
        } else if (m == u"console.save") {
            saved.insert(p.value(u"path"_s).toString(), p.value(u"text"_s).toString());
            reply(id, QJsonObject{});
        }
    }
public:
    QHash<QString, QString> saved;
    QStringList recorded; // serial.record's paths ("" to stop)
    QString recording;
    void pushRecordFailed() { send(event(u"serial.recordFailed"_s, {{u"message"_s, u"No space left on device"_s}, {u"bytes"_s, 7}})); }
private:

    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestSerialMonitor : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    // Keep QSettings away from the real configuration.
    void initTestCase()
    {
        QStandardPaths::setTestModeEnabled(true);
        // A name for QSettings, which on Windows keeps nothing without one.
        QCoreApplication::setOrganizationName(u"esp32bench-tests"_s);
        QCoreApplication::setApplicationName(u"tst_serialmonitor"_s);
        QSettings().remove(u"serial"_s);
    }
    // Each test starts with no remembered port or baud rate.
    void init() { QSettings().remove(u"serial"_s); }

    void opensShowsOutputAndSends()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        SerialMonitor *mon = window.serialMonitor();
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        // The Devices panel subscribes; the monitor's port list follows it.
        QTRY_COMPARE(mon->portPicker()->count(), 1);

        mon->openButton()->click();
        QTRY_VERIFY(core.isSerialOpen());
        QCOMPARE(fake.opened, QStringList{u"/dev/ttyUSB0@115200"_s});
        QTRY_VERIFY(mon->outputText().contains(u"café ok"_s));
        QVERIFY(mon->outputText().contains(u"ets Jul 29 2019\ncafé"_s)); // \r removed
        QVERIFY(mon->outputText().contains(u"restart when their port is opened"_s));

        mon->input()->setText(u"SET wifi_ssid home"_s);
        QTest::keyClick(mon->input(), Qt::Key_Return);
        QTRY_COMPARE(fake.written.size(), 1);
        QCOMPARE(fake.written.at(0), QByteArray("SET wifi_ssid home\n"));
        QVERIFY(mon->input()->text().isEmpty());

        // The status bar says which port is open.
        QCOMPARE(window.portStateText(), u"Serial: /dev/ttyUSB0 at 115200 baud"_s);

        // Find the speed: the monitor closes, the core tries the speeds, and it opens
        // again at the one that read as text.
        mon->findBaudButton()->click();
        QTRY_COMPARE(fake.opened.size(), 2);
        QCOMPARE(fake.opened.last(), u"/dev/ttyUSB0@9600"_s);
        QVERIFY(mon->outputText().contains(u"The board talks at 9600 baud."_s));
        QTRY_VERIFY(core.isSerialOpen());

        fake.pushClosed();
        QTRY_VERIFY(!core.isSerialOpen());
        QVERIFY(mon->outputText().contains(u"Disconnected"_s));
        QVERIFY(!mon->input()->isEnabled());
        QVERIFY(window.portStateText().isEmpty());
    }

    void recordsToAFile()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        SerialMonitor *mon = window.serialMonitor();
        core.start();
        QTRY_COMPARE(mon->portPicker()->count(), 1);
        QVERIFY(!mon->recordButton()->isEnabled()); // nothing open
        mon->openButton()->click();
        QTRY_VERIFY(core.isSerialOpen());
        QVERIFY(mon->recordButton()->isEnabled());

        mon->recordTo(u"/tmp/board.log"_s);
        QTRY_VERIFY(mon->recordButton()->isChecked());
        QCOMPARE(fake.recorded, QStringList{u"/tmp/board.log"_s});
        QVERIFY(mon->outputText().contains(u"Recording to /tmp/board.log"_s));
        // Stop: from the button, which needs no file then.
        mon->recordButton()->click();
        QTRY_VERIFY(!mon->recordButton()->isChecked());
        QCOMPARE(fake.recorded.last(), QString());
        QTRY_VERIFY(mon->outputText().contains(u"Recorded 42 bytes to /tmp/board.log."_s));

        // A failed write ends it, and says how far it got.
        mon->recordTo(u"/tmp/full.log"_s);
        QTRY_VERIFY(mon->recordButton()->isChecked());
        fake.pushRecordFailed();
        QTRY_VERIFY(!mon->recordButton()->isChecked());
        QVERIFY(mon->outputText().contains(u"No space left on device. 7 bytes were written to /tmp/full.log."_s));

        // Closing the port ends a recording too.
        mon->recordTo(u"/tmp/again.log"_s);
        QTRY_VERIFY(mon->recordButton()->isChecked());
        fake.pushClosed();
        QTRY_VERIFY(!mon->recordButton()->isChecked());
        QVERIFY(mon->outputText().contains(u"Stopped recording to /tmp/again.log."_s));
        QVERIFY(!mon->recordButton()->isEnabled());
    }

    void consoleStaysWhereTheReaderIs()
    {
        ConsoleView view;
        view.resize(300, 120);
        view.show();
        QVERIFY(QTest::qWaitForWindowExposed(&view));
        QScrollBar *bar = view.verticalScrollBar();
        for (int n = 0; n < 100; ++n)
            view.appendNotice(u"line %1"_s.arg(n));
        QVERIFY(bar->maximum() > 0);
        QCOMPARE(bar->value(), bar->maximum()); // at the end: it follows
        QVERIFY(view.newOutputButton()->isHidden());
        // Scrolled up: new output and notices leave the view where it is...
        bar->setValue(0);
        view.appendNotice(u"Simulator stopped"_s);
        QCOMPARE(bar->value(), 0);
        QVERIFY(!view.newOutputButton()->isHidden());
        // ...and the button goes to the end and away.
        view.newOutputButton()->click();
        QCOMPARE(bar->value(), bar->maximum());
        QVERIFY(view.newOutputButton()->isHidden());
        QVERIFY(view.toPlainText().endsWith(u"-- Simulator stopped\n"_s));
    }

    void consoleFindsStampsAndSaves()
    {
        ConsoleView view;
        view.appendBytes("boot\nready\n");
        view.setTimestamps(true);
        view.appendBytes("tick 1\ntick");
        view.appendBytes(" 2\n");
        const QStringList lines = view.toPlainText().split(u'\n');
        // Lines from before stay as they were; new ones start with this computer's time,
        // once each, even when a line arrives in pieces.
        QCOMPARE(lines.at(0), u"boot"_s);
        QVERIFY(QRegularExpression(u"^\\[\\d\\d:\\d\\d:\\d\\d\\.\\d{3}\\] tick 1$"_s).match(lines.at(2)).hasMatch());
        QVERIFY(QRegularExpression(u"^\\[[^\\]]+\\] tick 2$"_s).match(lines.at(3)).hasMatch());
        // Find wraps round, both ways.
        QVERIFY(view.findText(u"tick"_s));
        const int first = view.textCursor().selectionStart();
        QVERIFY(view.findText(u"tick"_s));
        QVERIFY(view.textCursor().selectionStart() > first);
        QVERIFY(view.findText(u"tick"_s)); // round to the first
        QCOMPARE(view.textCursor().selectionStart(), first);
        QVERIFY(view.findText(u"ready"_s, true));
        QVERIFY(!view.findText(u"nowhere"_s));

        // Save… hands the text to the core, which writes it, and says so.
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        ConsoleBar bar(&view, &core, u"test/timestamps"_s);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        const QString text = view.toPlainText();
        bar.saveTo(u"/tmp/out.txt"_s);
        QTRY_COMPARE(fake.saved.value(u"/tmp/out.txt"_s), text);
        QTRY_VERIFY(view.toPlainText().contains(u"Saved the output to /tmp/out.txt."_s));
    }

    void waitsForABoardThatComesBack()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SerialMonitor mon(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        mon.openPort(u"/dev/ttyUSB0"_s);
        QTRY_VERIFY(core.isSerialOpen());
        QVERIFY(fake.reconnect); // the core is asked to reopen it
        fake.pushLost();
        QTRY_VERIFY(mon.outputText().contains(u"waiting for it to come back"_s));
        QVERIFY(!mon.input()->isEnabled());
        fake.pushReconnected();
        QTRY_VERIFY(mon.outputText().contains(u"Reconnected."_s));
        QVERIFY(mon.input()->isEnabled());
        QVERIFY(core.isSerialOpen());
    }

    void showsAndSendsHex()
    {
        QCOMPARE(ConsoleView::hexText("Hi\n\x01"), u"48 69 0a\n01 "_s);
        QCOMPARE(SerialMonitor::hexBytes(u"01 a0 FF"_s), QByteArray("\x01\xa0\xff", 3));
        QCOMPARE(SerialMonitor::hexBytes(u"0x01a0"_s), QByteArray("\x01\xa0", 2));
        QVERIFY(SerialMonitor::hexBytes(u"1"_s).isEmpty());
        QVERIFY(SerialMonitor::hexBytes(u"zz"_s).isEmpty());

        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SerialMonitor mon(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        mon.openPort(u"/dev/ttyUSB0"_s);
        QTRY_VERIFY(core.isSerialOpen());
        mon.hexBox()->setChecked(true);
        mon.input()->setText(u"de ad"_s);
        QTest::keyClick(mon.input(), Qt::Key_Return);
        QTRY_COMPARE(fake.written.size(), 1);
        QCOMPARE(fake.written.at(0), QByteArray("\xde\xad", 2)); // no line ending in hex
        mon.input()->setText(u"nope"_s);
        QTest::keyClick(mon.input(), Qt::Key_Return);
        QTRY_VERIFY(mon.outputText().contains(u"Not hex bytes"_s));
        QCOMPARE(fake.written.size(), 1);
        fake.pushData("A\n");
        QTRY_VERIFY(mon.outputText().contains(u"41 0a"_s));
    }

    void coloursAndFiltersLogLines()
    {
        ConsoleView view;
        QSignalSpy tags(&view, &ConsoleView::tagsChanged);
        // As the core sends ESP-IDF's log: the text without colour codes, and the
        // finished lines' levels and tags; a line may finish in a later chunk.
        view.appendOutput("raw", u"I (1) wifi: up\nE (2) app: no sensor\nhello\nW (3) wi"_s,
                          QJsonArray{QJsonObject{{u"level"_s, u"info"_s}, {u"tag"_s, u"wifi"_s}},
                                     QJsonObject{{u"level"_s, u"error"_s}, {u"tag"_s, u"app"_s}}, QJsonObject{}});
        view.appendOutput("raw", u"fi: slow\n"_s, QJsonArray{QJsonObject{{u"level"_s, u"warning"_s}, {u"tag"_s, u"wifi"_s}}});
        view.appendNotice(u"Saved."_s);
        QCOMPARE(view.tags(), (QStringList{u"wifi"_s, u"app"_s}));
        QVERIFY(tags.size() >= 1);
        const QTextBlock error = view.document()->findBlockByNumber(1);
        QCOMPARE(error.userState(), int(OutputLines::Error));
        QCOMPARE(QTextCursor(error).charFormat().foreground().color(), OutputLines::colour(OutputLines::Error, view.palette()));

        view.setLevelFilter(ConsoleView::Problems);
        QCOMPARE(view.visibleLines(), (QStringList{u"E (2) app: no sensor"_s, u"W (3) wifi: slow"_s, u"-- Saved."_s}));
        view.setLevelFilter(ConsoleView::ErrorsOnly);
        QCOMPARE(view.visibleLines(), (QStringList{u"E (2) app: no sensor"_s, u"-- Saved."_s}));
        view.setLevelFilter(ConsoleView::AllLines);
        view.setTagFilter(u"wifi"_s);
        QCOMPARE(view.visibleLines(), (QStringList{u"I (1) wifi: up"_s, u"W (3) wifi: slow"_s, u"-- Saved."_s}));
        view.setTagFilter({});
        QCOMPARE(view.visibleLines().size(), 5);

        // In hex, or from a core that sends no `shown`, the raw bytes are shown.
        ConsoleView raw;
        raw.appendOutput("ab\n", QJsonValue(), {});
        QCOMPARE(raw.toPlainText(), u"ab\n"_s);
    }

    void reopensTheLastPortAtStart()
    {
        QSettings settings;
        settings.setValue(u"serial/lastPort"_s, u"/dev/ttyUSB0"_s);
        settings.setValue(u"serial/lastBaud"_s, 9600);
        settings.setValue(u"serial/open"_s, true);
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SerialMonitor mon(&core);
        QCOMPARE(mon.baudPicker()->currentData().toInt(), 9600);
        mon.restoreLastPort();
        core.watchDevices(); // no-op until connected; MainWindow subscribes on connect
        connect(&core, &CoreConnection::stateChanged, &core, [&core](CoreConnection::State s) {
            if (s == CoreConnection::State::Connected)
                core.watchDevices();
        });
        core.start();
        QTRY_COMPARE(fake.opened, QStringList{u"/dev/ttyUSB0@9600"_s});
        // Closed on purpose: not reopened next time.
        mon.openButton()->click();
        QTRY_VERIFY(!core.isSerialOpen());
        QCOMPARE(settings.value(u"serial/open"_s).toBool(), false);
        settings.remove(u"serial"_s);
    }

    void windowExtras()
    {
        QSettings().remove(u"app/lastVersion"_s);
        CoreConnection core;
        MainWindow window(&core);
        // A first start says nothing; after an upgrade, once.
        window.sayWhatsNew();
        QVERIFY(window.messageBar()->text().isEmpty());
        QSettings().setValue(u"app/lastVersion"_s, u"0.0.1"_s);
        window.sayWhatsNew();
        QVERIFY(window.messageBar()->text().contains(u"(it was 0.0.1)"_s));
        window.messageBar()->hide();
        window.sayWhatsNew();
        QVERIFY(window.messageBar()->text().isEmpty());

        // Copy details: the error and the versions.
        window.messageBar()->showError(u"Flashing failed: no reply"_s);
        QVERIFY(!window.messageBar()->copyButton()->isHidden());
        QVERIFY(window.messageBar()->detailsText().startsWith(u"Flashing failed: no reply\n\n"_s));
        QVERIFY(window.messageBar()->detailsText().contains(u"- App: "_s));
        window.messageBar()->dismiss();

        // Preferences: the colours, where this Qt can choose them.
        if (Preferences::canChooseTheme()) {
            window.showPreferences();
            QVERIFY(window.preferences()->themePicker()->count() == 3);
            window.preferences()->reject();
        }

        // The Problems dock keeps every explanation.
        emit core.outputExplained(u"serial"_s, QJsonArray{QJsonObject{{u"level"_s, u"problem"_s},
                                                                     {u"text"_s, u"The board browned out."_s},
                                                                     {u"url"_s, u"https://esp32bench.dev/errors/brownout/"_s}}});
        QCOMPARE(window.problems()->tree()->topLevelItemCount(), 1);
        bool titled = false;
        for (QDockWidget *d : window.findChildren<QDockWidget *>())
            titled |= d->windowTitle() == u"Problems (1)"_s;
        QVERIFY(titled);
        QCOMPARE(window.problems()->tree()->topLevelItem(0)->text(ProblemsPanel::TextColumn), u"The board browned out."_s);
        // Only one kind of problem, and clearing just those.
        emit core.simBootLoop(QJsonObject{{u"resets"_s, 3}, {u"seconds"_s, 4}});
        const int before = window.problems()->tree()->topLevelItemCount();
        window.problems()->showOnly(u"serial"_s);
        int shown = 0;
        for (int i = 0; i < before; ++i)
            shown += window.problems()->tree()->topLevelItem(i)->isHidden() ? 0 : 1;
        QVERIFY(shown < before);
        window.problems()->clearButton()->click();
        QCOMPARE(window.problems()->tree()->topLevelItemCount(), before - shown);
        window.problems()->showOnly({});
        // The interface size, read before the app starts.
        QSettings().setValue(u"ui/size"_s, 150);
        QCOMPARE(Preferences::interfaceSize(), 150);
        QSettings().setValue(u"ui/size"_s, 9000);
        QCOMPARE(Preferences::interfaceSize(), 300);
        QSettings().remove(u"ui/size"_s);
        // Looking an error up on the site sends its own words only.
        QCOMPARE(ProblemsPanel::searchTerms(u"Guru Meditation Error: Core  1 panic'ed (LoadProhibited). Exception was unhandled."_s),
                 u"LoadProhibited"_s);
        QCOMPARE(ProblemsPanel::searchTerms(u"error: 'foo' was not declared in this scope"_s), u"was not declared in this scope"_s);
        QCOMPARE(ProblemsPanel::searchUrl(u"SW_CPU_RESET"_s).toString(), u"https://esp32bench.dev/search/?q=SW_CPU_RESET"_s);
        // Alt+1 to Alt+9 go to the panels; the speed of a simulation, in words.
        QStringList goTo;
        for (QMenu *menu : window.menuBar()->findChildren<QMenu *>())
            for (QAction *a : menu->actions())
                if (a->shortcut() == QKeySequence(Qt::ALT | Qt::Key_3))
                    goTo << a->text();
        QCOMPARE(goTo, QStringList{u"Simulator"_s});
        window.goToPanel(window.findChild<QDockWidget *>(u"traceDock"_s));
        QVERIFY(window.findChild<QDockWidget *>(u"traceDock"_s)->isVisibleTo(&window));
        QCOMPARE(MainWindow::speedText(0.87), u"%1× real time"_s.arg(QLocale().toString(0.9, 'f', 1)));
        // The layouts: the panels each task needs.
        window.showLayout(MainWindow::Layout::Hardware);
        QVERIFY(window.findChild<QDockWidget *>(u"serialDock"_s)->isVisibleTo(&window));
        QVERIFY(!window.findChild<QDockWidget *>(u"debugDock"_s)->isVisibleTo(&window));
        window.showLayout(MainWindow::Layout::Debugging);
        QVERIFY(window.findChild<QDockWidget *>(u"debugDock"_s)->isVisibleTo(&window));
        QVERIFY(!window.findChild<QDockWidget *>(u"serialDock"_s)->isVisibleTo(&window));
        // A decoded crash: where it was, the sketch's own line first; double-click shows it.
        emit core.simBacktrace(QJsonArray{QJsonObject{{u"addr"_s, u"0x40001234"_s}, {u"func"_s, u"abort"_s}},
                                          QJsonObject{{u"addr"_s, u"0x400d2a5e"_s}, {u"func"_s, u"loop()"_s},
                                                      {u"file"_s, u"/s/blink/blink.ino"_s}, {u"line"_s, 12},
                                                      {u"sketch"_s, true}}});
        QTreeWidgetItem *crash = window.problems()->tree()->topLevelItem(1);
        QVERIFY(crash);
        QCOMPARE(crash->text(ProblemsPanel::TextColumn), u"Crashed in loop() (blink.ino, line 12)"_s);
        QCOMPARE(crash->text(ProblemsPanel::SourceColumn), u"Simulator"_s);
        QSignalSpy crashAt(window.problems(), &ProblemsPanel::locationActivated);
        emit window.problems()->tree()->itemActivated(crash, ProblemsPanel::TextColumn);
        QCOMPARE(crashAt.size(), 1);
        QCOMPARE(crashAt.at(0).at(0).toJsonObject().value(u"path"_s).toString(), u"/s/blink/blink.ino"_s);
        emit core.serialBacktrace(QJsonArray{QJsonObject{{u"addr"_s, u"0x40001234"_s}}});
        QCOMPARE(window.problems()->tree()->topLevelItem(2)->text(ProblemsPanel::TextColumn), u"Crashed in 0x40001234"_s);

        // Undo in the message bar.
        bool undone = false;
        window.messageBar()->showUndo(u"Forgot x."_s, u"Undo"_s, [&undone] { undone = true; });
        window.messageBar()->actionButton()->click();
        QVERIFY(undone);
        QVERIFY(window.messageBar()->text().isEmpty());

        // Toolbar buttons say their shortcuts; the title says nothing is open.
        bool found = false;
        for (QAction *a : window.toolBar()->actions())
            found |= a->toolTip().contains(QKeySequence(Qt::CTRL | Qt::Key_B).toString(QKeySequence::NativeText));
        QVERIFY(found);
        QCOMPARE(window.windowTitle(), u"ESP32 Bench"_s);

        // Ctrl+K: actions found by typing, run with Enter.
        window.showCommandPalette();
        QTest::keyClicks(window.commandPalette()->field(), u"prefer"_s);
        QVERIFY(window.commandPalette()->shown().contains(u"Preferences…"_s));
        window.commandPalette()->runSelected();
        QTRY_VERIFY(window.preferences() && window.preferences()->isVisible());
        window.preferences()->reject();

        // A real board's output, checked against the project flashed to it.
        emit core.serialExpect({}, {u"tick \\d+ ms"_s}, u"none"_s, 8, false);
        QVERIFY(window.serialMonitor()->expectText().contains(u"0 of 1"_s));
        emit core.serialExpect({u"tick \\d+ ms"_s}, {}, u"none"_s, 8, false);
        QVERIFY(window.serialMonitor()->expectText().contains(u"✓"_s));

        // Preferences: the consoles follow the text size, and Cancel puts it back.
        QSettings().setValue(u"console/zoom"_s, 0);
        window.showPreferences();
        window.preferences()->textSize()->setValue(3);
        QCOMPARE(window.serialMonitor()->console()->zoomSteps(), 3);
        window.preferences()->reject();
        QCOMPARE(window.serialMonitor()->console()->zoomSteps(), 0);
        window.showPreferences();
        window.preferences()->textSize()->setValue(2);
        window.preferences()->languagePicker()->setCurrentIndex(window.preferences()->languagePicker()->findData(u"de"_s));
        window.preferences()->releaseNoticesBox()->setChecked(false);
        window.preferences()->save();
        QCOMPARE(Preferences::language(), u"de"_s);
        QCOMPARE(QSettings().value(u"console/zoom"_s).toInt(), 2);
        QVERIFY(!Preferences::releaseNotices());
        QSettings().remove(u"ui"_s);
        QSettings().remove(u"update"_s);
        QSettings().setValue(u"console/zoom"_s, 0);
    }

    void quickSendButtons()
    {
        CoreConnection core;
        SerialMonitor mon(&core);
        mon.setProject(u"simon"_s);
        emit core.commandsListed(u"simon"_s, {u"help"_s, u"status"_s});
        QCOMPARE(mon.quickCommands(), (QStringList{u"help"_s, u"status"_s}));
        QPushButton *help = nullptr;
        for (QPushButton *b : mon.findChildren<QPushButton *>())
            if (b->text() == u"help"_s)
                help = b;
        QVERIFY(help);
        QVERIFY(!help->isEnabled()); // the port is closed
        // Another project's lines are not this one's.
        emit core.commandsListed(u"blink"_s, {u"x"_s});
        QCOMPARE(mon.quickCommands().size(), 2);
        mon.setProject(u"blink"_s);
        QVERIFY(mon.quickCommands().isEmpty());
    }

    void offersTheUsualSpeedWhenGarbled()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SerialMonitor mon(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        mon.baudPicker()->setCurrentIndex(mon.baudPicker()->findData(9600));
        mon.openPort(u"/dev/ttyUSB0"_s);
        QTRY_VERIFY(core.isSerialOpen());
        emit core.serialGarbled(9600, 115200);
        QVERIFY(!mon.garbledSwitch()->isHidden());
        mon.garbledSwitch()->click();
        QTRY_COMPARE(fake.opened.last(), u"/dev/ttyUSB0@115200"_s);
        QVERIFY(mon.garbledSwitch()->isHidden());
    }

    void consoleZoomsAndInputRemembers()
    {
        ConsoleView view;
        view.resetZoom();
        const qreal base = view.font().pointSizeF();
        view.zoom(2);
        QCOMPARE(view.font().pointSizeF(), base + 2);
        // Kept for the next console.
        ConsoleView other;
        QCOMPARE(other.zoomSteps(), 2);
        view.resetZoom();
        QCOMPARE(view.font().pointSizeF(), base);

        QLineEdit input;
        auto *history = new InputHistory(&input);
        for (const QString &line : {u"help"_s, u"status"_s}) {
            input.setText(line);
            QTest::keyClick(&input, Qt::Key_Return);
            input.clear(); // as the panel does after sending
        }
        QCOMPARE(history->lines(), (QStringList{u"help"_s, u"status"_s}));
        input.setText(u"draft"_s);
        QTest::keyClick(&input, Qt::Key_Up);
        QCOMPARE(input.text(), u"status"_s);
        QTest::keyClick(&input, Qt::Key_Up);
        QCOMPARE(input.text(), u"help"_s);
        QTest::keyClick(&input, Qt::Key_Up);
        QCOMPARE(input.text(), u"help"_s);
        QTest::keyClick(&input, Qt::Key_Down);
        QCOMPARE(input.text(), u"status"_s);
        QTest::keyClick(&input, Qt::Key_Down);
        QCOMPARE(input.text(), u"draft"_s);
    }

    void windowHelpsDropsAndNotices()
    {
        CoreConnection core;
        MainWindow window(&core);
        // Drops: project folders and esp32bench:// links; web links are not ours.
        QMimeData mime;
        mime.setUrls({QUrl::fromLocalFile(u"/home/someone/blinky"_s), QUrl(u"esp32bench://project/blink"_s),
                      QUrl(u"https://example.com/"_s)});
        QCOMPARE(MainWindow::dropTargets(&mime),
                 (QStringList{u"/home/someone/blinky"_s, u"esp32bench://project/blink"_s}));
        QMimeData text;
        text.setText(u"esp32bench://project/simon?board=esp32"_s);
        QCOMPARE(MainWindow::dropTargets(&text), QStringList{u"esp32bench://project/simon?board=esp32"_s});
        QMimeData other;
        other.setText(u"hello"_s);
        QVERIFY(MainWindow::dropTargets(&other).isEmpty());

        // A problem report says the versions and nothing about the user's files.
        const QUrl report = window.problemReportUrl();
        QCOMPARE(report.host(), u"github.com"_s);
        const QString body = QUrlQuery(report).queryItemValue(u"body"_s, QUrl::FullyDecoded);
        QVERIFY(body.contains(u"- App: "_s));
        QVERIFY(body.contains(u"- System: "_s));
        QVERIFY(!body.contains(QDir::homePath()));

        // The shortcuts list has the toolbar's steps and the console's keys.
        const QString keys = window.shortcutsText();
        QVERIFY(keys.contains(QKeySequence(Qt::CTRL | Qt::Key_B).toString(QKeySequence::NativeText)));
        QVERIFY(keys.contains(u"lines sent before"_s));

        // A newer release is said once, and not again once closed.
        QSettings().remove(u"update/dismissed"_s);
        const QJsonObject library{{u"available"_s, true}, {u"projects"_s, QJsonArray{}},
                                  {u"update"_s, QJsonObject{{u"version"_s, u"9.0.0"_s},
                                                            {u"url"_s, u"https://esp32bench.dev/desktop/"_s}}}};
        emit core.libraryListed(library);
        QVERIFY(window.messageBar()->text().contains(u"9.0.0"_s));
        window.messageBar()->findChild<QToolButton *>()->click();
        QCOMPARE(QSettings().value(u"update/dismissed"_s).toString(), u"9.0.0"_s);
        MainWindow again(&core);
        emit core.libraryListed(library);
        QVERIFY(again.messageBar()->text().isEmpty());
        QSettings().remove(u"update/dismissed"_s);
    }

    void doubleClickingAPortOpensIt()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        core.start();
        QTRY_COMPARE(window.devicesPanel()->tree()->topLevelItemCount(), 1);
        QTreeWidgetItem *item = window.devicesPanel()->tree()->topLevelItem(0);
        emit window.devicesPanel()->tree()->itemActivated(item, 0);
        QTRY_VERIFY(core.isSerialOpen());
        QCOMPARE(fake.opened, QStringList{u"/dev/ttyUSB0@115200"_s});
    }
};

QTEST_MAIN(TestSerialMonitor)
#include "tst_serialmonitor.moc"
