// SetupDialog and the first-run check against a fake core.

#include "CoreConnection.h"
#include "MainWindow.h"
#include "SetupDialog.h"

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPlainTextEdit>
#include <QPointer>
#include <QPushButton>
#include <QSettings>
#include <QStandardPaths>
#include <QTemporaryDir>
#include <QLocale>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

static QJsonArray components(bool coreInstalled)
{
    QJsonObject cli{{u"id"_s, u"arduino-cli"_s}, {u"name"_s, u"arduino-cli"_s}, {u"neededFor"_s, u"build"_s},
                    {u"state"_s, u"installed"_s}, {u"version"_s, u"1.5.1"_s}, {u"installable"_s, false}};
    QJsonObject core{{u"id"_s, u"esp32-core"_s}, {u"name"_s, u"ESP32 Arduino core"_s}, {u"neededFor"_s, u"build"_s},
                     {u"state"_s, coreInstalled ? u"installed"_s : u"missing"_s}, {u"wanted"_s, u"3.3.12"_s},
                     {u"installable"_s, true}};
    if (coreInstalled)
        core[u"version"_s] = u"3.3.12"_s;
    else
        core[u"downloadBytes"_s] = 1810706323;
    QJsonObject qemu{{u"id"_s, u"qemu-xtensa"_s}, {u"name"_s, u"QEMU for ESP32 and ESP32-S3"_s},
                     {u"neededFor"_s, u"simulator"_s}, {u"state"_s, u"installed"_s},
                     {u"version"_s, u"esp-develop-9.2.2-20261109"_s}, {u"wanted"_s, u"esp-develop-9.2.2-20261109"_s},
                     {u"installable"_s, true}};
    return {cli, core, qemu};
}

// Answers core.hello, device.watch, toolchain.status (core missing until installed)
// and toolchain.install (progress sent before the reply).
class FakeCore : public QObject {
    Q_OBJECT
public:
    bool coreInstalled = false;
    bool slow = false; // installs never finish by themselves
    QStringList installs;
    QList<qint64> cancelled;

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
    void event(const QString &m, QJsonObject p)
    {
        p[u"subscription"_s] = 6;
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, m}, {u"params"_s, p}});
    }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        if (m == u"core.hello")
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.41.0"_s}});
        else if (m == u"device.watch")
            reply(id, QJsonObject{{u"subscription"_s, 1}, {u"ports"_s, QJsonArray{}}});
        else if (m == u"library.list")
            reply(id, QJsonObject{{u"available"_s, true}, {u"source"_s, u"x"_s}, {u"projects"_s, QJsonArray{}}});
        else if (m == u"toolchain.status")
            reply(id, QJsonObject{{u"components"_s, components(coreInstalled)}});
        else if (m == u"job.cancel") {
            cancelled << req.value(u"params"_s).toObject().value(u"job"_s).toInteger();
            reply(id, QJsonObject{});
        } else if (m == u"toolchain.install" && slow) {
            // Still downloading: no finished event until cancelled.
            installs << req.value(u"params"_s).toObject().value(u"component"_s).toString();
            event(u"toolchain.progress"_s, {{u"line"_s, u"Downloading..."_s}});
            reply(id, QJsonObject{{u"job"_s, 6}});
        } else if (m == u"toolchain.install") {
            installs << req.value(u"params"_s).toObject().value(u"component"_s).toString();
            event(u"toolchain.progress"_s, {{u"line"_s, u"Installing esp32:esp32@3.3.12..."_s}});
            coreInstalled = true;
            event(u"toolchain.finished"_s, {{u"ok"_s, true}});
            reply(id, QJsonObject{{u"job"_s, 6}});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestSetup : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void initTestCase()
    {
        // An organisation, so QSettings can save: on Windows it is the registry key
        // Software\<organisation>\<app>, which an empty name does not make.
        QCoreApplication::setOrganizationName(u"esp32bench-tests"_s);
        QStandardPaths::setTestModeEnabled(true);
        QSettings().clear();
    }

    void showsTheDownloadAndTimeLeft()
    {
        CoreConnection core;
        SetupDialog dialog(&core);
        QVERIFY(dialog.downloadText().isEmpty());
        dialog.showDownload(100'000'000, 1'000'000'000);
        QVERIFY(dialog.downloadText().contains(u"of"_s));
        QTest::qWait(1100);
        dialog.showDownload(200'000'000, 1'000'000'000);
        // 100 MB in a second: about 8 s to go.
        QVERIFY2(dialog.downloadText().contains(u"left"_s), qPrintable(dialog.downloadText()));
        emit core.toolchainInstallFinished(true, {});
        QVERIFY(dialog.downloadText().isEmpty());
    }

    void rendersStatus()
    {
        CoreConnection core;
        SetupDialog dialog(&core);
        dialog.showStatus(components(false));
        QTreeWidget *t = dialog.tree();
        QCOMPARE(t->topLevelItemCount(), 3);
        QCOMPARE(t->topLevelItem(0)->text(SetupDialog::StateColumn), u"Installed"_s);
        QTreeWidgetItem *esp = t->topLevelItem(1);
        QCOMPARE(esp->text(SetupDialog::StateColumn), u"Missing"_s);
        QCOMPARE(esp->text(SetupDialog::VersionColumn), u"needs 3.3.12"_s);
        QVERIFY(!esp->text(SetupDialog::SizeColumn).isEmpty());
        QVERIFY(qobject_cast<QPushButton *>(t->itemWidget(esp, SetupDialog::ActionColumn)));
        // Installed rows get no button.
        QVERIFY(!t->itemWidget(t->topLevelItem(0), SetupDialog::ActionColumn));
    }

    void installsEverythingNeededInOneStep()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        SetupDialog dialog(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.requestToolchainStatus();
        // What it will fetch in all, in one button.
        QTRY_VERIFY(dialog.installAllButton()->isVisibleTo(&dialog));
        QVERIFY2(dialog.installAllButton()->text().contains(QLocale().formattedDataSize(1810706323)),
                 qPrintable(dialog.installAllButton()->text()));
        dialog.installAllButton()->click();
        QTRY_COMPARE(fake.installs, QStringList{u"esp32-core"_s});
        // Done: nothing left to install, so the button goes.
        QTRY_VERIFY(!dialog.installAllButton()->isVisibleTo(&dialog));
        QVERIFY(dialog.logText().contains(u"Done."_s));
    }

    void cancelsAnInstall()
    {
        FakeCore fake(path());
        fake.slow = true;
        CoreConnection core;
        core.setSocketPath(path());
        SetupDialog dialog(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.requestToolchainStatus();
        QTRY_VERIFY(dialog.installAllButton()->isVisibleTo(&dialog));
        QVERIFY(!dialog.cancelButton()->isVisibleTo(&dialog));
        dialog.installAllButton()->click();
        QTRY_VERIFY(core.isInstalling());
        QTRY_VERIFY(dialog.logText().contains(u"Downloading..."_s));
        QVERIFY(dialog.cancelButton()->isVisibleTo(&dialog));
        dialog.cancelButton()->click();
        QTRY_COMPARE(fake.cancelled, QList<qint64>{6});
        QTRY_VERIFY(!core.isInstalling());
        QVERIFY(dialog.logText().contains(u"Install failed: Cancelled."_s));
        QVERIFY(!dialog.cancelButton()->isVisibleTo(&dialog));
        QCOMPARE(fake.installs, QStringList{u"esp32-core"_s}); // nothing more after it
    }

    // With ESP32BENCH_SETUP_STATUS (a toolchain.status components file) and
    // ESP32BENCH_SCREENSHOT set, render that status to a picture for a visual check.
    void screenshotFromRealStatus()
    {
        const QString statusFile = qEnvironmentVariable("ESP32BENCH_SETUP_STATUS");
        const QString shot = qEnvironmentVariable("ESP32BENCH_SCREENSHOT");
        if (statusFile.isEmpty() || shot.isEmpty())
            QSKIP("ESP32BENCH_SETUP_STATUS and ESP32BENCH_SCREENSHOT not set");
        QFile f(statusFile);
        QVERIFY(f.open(QIODevice::ReadOnly));
        CoreConnection core;
        SetupDialog dialog(&core);
        dialog.showStatus(QJsonDocument::fromJson(f.readAll()).array());
        dialog.show();
        QVERIFY(dialog.grab().save(shot));
    }

    void showsTheManualFixWhenTheAppCannotInstall()
    {
        CoreConnection core;
        SetupDialog dialog(&core);
        QJsonArray c = components(false);
        QJsonObject cli = c.at(0).toObject();
        cli[u"state"_s] = u"missing"_s;
        cli.remove(u"version"_s);
        cli[u"hint"_s] = u"Install it from the Arch repositories: sudo pacman -S arduino-cli"_s;
        c[0] = cli;
        dialog.showStatus(c);
        QTreeWidgetItem *row = dialog.tree()->topLevelItem(0);
        QVERIFY(!dialog.tree()->itemWidget(row, SetupDialog::ActionColumn));
        QVERIFY(row->text(SetupDialog::ActionColumn).contains(u"pacman -S arduino-cli"_s));
    }

    void firstRunOpensSetupAndInstallUpdatesIt()
    {
        QSettings().clear();
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        core.start();
        // The core is missing, so the setup opens by itself.
        QTRY_VERIFY(window.setupDialog()->isVisible());
        QTRY_COMPARE(window.setupDialog()->tree()->topLevelItemCount(), 3);

        QTreeWidget *t = window.setupDialog()->tree();
        auto *button = qobject_cast<QPushButton *>(t->itemWidget(t->topLevelItem(1), SetupDialog::ActionColumn));
        QVERIFY(button);
        button->click();
        QTRY_COMPARE(fake.installs, QStringList{u"esp32-core"_s});
        QTRY_VERIFY(window.setupDialog()->logText().contains(u"Installing esp32:esp32@3.3.12"_s));
        QTRY_VERIFY(window.setupDialog()->logText().contains(u"Done."_s));
        // The status is checked again and now shows it installed.
        QTRY_COMPARE(t->topLevelItem(1)->text(SetupDialog::StateColumn), u"Installed"_s);
        window.setupDialog()->hide();
    }

    void doesNotNagOnLaterRuns()
    {
        // The previous test offered the setup once; it stays closed now.
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        MainWindow window(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QTest::qWait(200);
        QVERIFY(!window.setupDialog()->isVisible());
    }
};

QTEST_MAIN(TestSetup)
#include "tst_setup.moc"
