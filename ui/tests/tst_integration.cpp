// The UI against a real esp32benchd. Skipped unless ESP32BENCH_DAEMON points at the
// daemon binary. Set ESP32BENCH_SCREENSHOT to a .png path to save the main window.

#include "CoreConnection.h"
#include "DevicesPanel.h"
#include "LibraryPanel.h"
#include "ProjectView.h"
#include "MainWindow.h"

#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTreeWidget>

#include <csignal>

using namespace Qt::StringLiterals;

class TestIntegration : public QObject {
    Q_OBJECT

private slots:
    void startsCoreAndListsDevices()
    {
        if (qEnvironmentVariableIsEmpty("ESP32BENCH_DAEMON"))
            QSKIP("ESP32BENCH_DAEMON is not set");

        // A private socket, so the test never talks to the user's own daemon.
        QTemporaryDir dir;
        const QString sock = dir.filePath(u"rt/core.sock"_s);

        // The daemon inherits these: a private cache, and the schema's example manifest
        // as the library source, so the test needs no network.
        qputenv("XDG_CACHE_HOME", dir.filePath(u"cache"_s).toLocal8Bit());
        qputenv("ESP32BENCH_MANIFEST",
                QByteArray(ESP32BENCH_SOURCE_DIR "/../schema/project-manifest/example.json"));

        CoreConnection core;
        core.setSocketPath(sock);
        MainWindow window(&core);
        window.show();
        core.start();
        QCOMPARE(core.state(), CoreConnection::State::Disconnected);

        QSignalSpy listed(&core, &CoreConnection::devicesListed);
        QVERIFY2(core.startCore(), qPrintable(core.lastError()));
        QTRY_COMPARE_WITH_TIMEOUT(core.state(), CoreConnection::State::Connected, 10000);
        QVERIFY(window.connectionText().contains(u"Connected"_s));
        // The panel asks for devices as soon as the core connects.
        QTRY_VERIFY_WITH_TIMEOUT(listed.size() >= 1, 5000);
        QVERIFY(!window.devicesPanel()->statusText().contains(u"Not connected"_s));
        // No library cached yet, so the panel downloads it once by itself.
        QTRY_COMPARE_WITH_TIMEOUT(window.libraryPanel()->tree()->topLevelItemCount(), 2, 10000);
        QVERIFY(window.libraryPanel()->statusText().contains(u"Projects: 2"_s));
        // Picking a project loads it through library.get.
        window.libraryPanel()->tree()->setCurrentItem(window.libraryPanel()->tree()->topLevelItem(0));
        QTRY_VERIFY_WITH_TIMEOUT(!window.projectView()->title().isEmpty(), 5000);
        QCOMPARE(window.projectView()->title(),
                 window.libraryPanel()->tree()->topLevelItem(0)->text(LibraryPanel::NameColumn));

        const QString shot = qEnvironmentVariable("ESP32BENCH_SCREENSHOT");
        if (!shot.isEmpty())
            QVERIFY(window.grab().save(shot));

        // Stop the daemon we started: it removes its socket on SIGTERM.
        QVERIFY(core.startedCorePid() > 0);
        QCOMPARE(::kill(pid_t(core.startedCorePid()), SIGTERM), 0);
        QTRY_VERIFY_WITH_TIMEOUT(core.state() == CoreConnection::State::Disconnected, 5000);
    }
};

QTEST_MAIN(TestIntegration)
#include "tst_integration.moc"
