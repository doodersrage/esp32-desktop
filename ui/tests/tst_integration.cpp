// The UI against a real esp32benchd. Skipped unless ESP32BENCH_DAEMON points at the
// daemon binary. Set ESP32BENCH_SCREENSHOT to a .png path to save the main window.

#include "CoreConnection.h"
#include "DebugPanel.h"
#include "DisplayPanel.h"
#include "DevicesPanel.h"
#include "BuildPanel.h"
#include "LibraryPanel.h"
#include "ProjectView.h"
#include "SimulatorPanel.h"
#include "MainWindow.h"

#include <QDir>
#include <QFile>
#include <QScopeGuard>
#include <QPushButton>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QStandardPaths>
#include <QTest>
#include <QLineEdit>
#include <QListWidget>
#include <QTreeWidget>

#include <csignal>

using namespace Qt::StringLiterals;

// Agrees to every build, as a user clicking "Build" in the consent dialog would.
class AgreeingWindow : public MainWindow {
public:
    using MainWindow::MainWindow;
    int asked = 0;
    bool confirmBuild(const QString &, const QString &, const QJsonObject &) override
    {
        ++asked;
        return true;
    }
};

// A stand-in arduino-cli for the daemon: "compiles" by writing empty outputs.
static QString writeFakeArduinoCli(const QString &dir)
{
    const QString path = dir + u"/fake-arduino-cli"_s;
    QFile f(path);
    if (!f.open(QIODevice::WriteOnly))
        qFatal("cannot write %s", qPrintable(path));
    f.write(R"SH(#!/bin/sh
case "$1" in
  lib) echo "Installed $3" ;;
  compile)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "--output-dir" ] && out="$a"; prev="$a"; last="$a"; done
    name=$(basename "$last")
    echo "Sketch uses 1234 bytes"
    : > "$out/$name.ino.merged.bin"; : > "$out/$name.ino.elf" ;;
esac
)SH");
    f.close();
    f.setPermissions(f.permissions() | QFileDevice::ExeOwner);
    return path;
}

class TestIntegration : public QObject {
    Q_OBJECT

private slots:
    // Keep QSettings away from the real configuration.
    void initTestCase() { QStandardPaths::setTestModeEnabled(true); }

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
        qputenv("XDG_DATA_HOME", dir.filePath(u"data"_s).toLocal8Bit());
        // The private data dir has no QEMU; point the daemon at the user's pinned one.
        const QString qemuBin = QDir::homePath()
            + u"/.local/share/esp32bench/qemu/esp-develop-9.2.2-20261109/xtensa/qemu/bin"_s;
        qputenv("ESP32BENCH_QEMU_DIR", qemuBin.toLocal8Bit());
        qputenv("ESP32BENCH_ARDUINO_CLI", writeFakeArduinoCli(dir.path()).toLocal8Bit());
        qputenv("ESP32BENCH_MANIFEST",
                QByteArray(ESP32BENCH_SOURCE_DIR "/../schema/project-manifest/example.json"));

        CoreConnection core;
        core.setSocketPath(sock);
        AgreeingWindow window(&core);
        window.show();
        QSignalSpy listed(&core, &CoreConnection::devicesListed);
        QSignalSpy autoStartFailed(&core, &CoreConnection::autoStartFailed);
        // As the app does: no core is running, so the first attempt starts one.
        core.setAutoStart(true);
        core.start();
        // Stop the daemon we started however the test ends.
        const auto stopDaemon = qScopeGuard([&core] {
            if (core.startedCorePid() > 0)
                ::kill(pid_t(core.startedCorePid()), SIGTERM);
        });
        QTRY_COMPARE_WITH_TIMEOUT(core.state(), CoreConnection::State::Connected, 10000);
        QVERIFY(window.connectionText().contains(u"Connected"_s));
        QCOMPARE(autoStartFailed.size(), 0);
        QVERIFY(core.startedCorePid() > 0);
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

        // Build it: the daemon asks for consent, the window agrees, the build runs.
        QPushButton *build = window.projectView()->buildButton();
        QTRY_VERIFY(build->isEnabled());
        QSignalSpy finished(&core, &CoreConnection::buildFinished);
        build->click();
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, 10000);
        QCOMPARE(window.asked, 1);
        QVERIFY2(finished.at(0).at(0).toBool(), qPrintable(finished.at(0).at(1).toString()));
        QVERIFY(window.buildPanel()->logText().contains(u"Sketch uses 1234 bytes"_s));

        // Run it in the simulator, with the real hello firmware standing in for the
        // stand-in compiler's empty image: UI -> daemon -> QEMU.
        const QString hello = QStringLiteral(ESP32BENCH_SOURCE_DIR "/../tests/firmware/out/hello-esp32.bin");
        const QString image = dir.filePath(u"cache/esp32bench/builds/blink/esp32/out/blink.ino.merged.bin"_s);
        if (QFile::exists(hello) && QFile::exists(qemuBin + u"/qemu-system-xtensa"_s)) {
            QVERIFY(QFile::remove(image));
            QVERIFY(QFile::copy(hello, image));
            QPushButton *run = window.buildPanel()->simulateButton();
            QVERIFY(run->isEnabled());
            run->click();
            QTRY_VERIFY_WITH_TIMEOUT(window.simulator()->outputText().contains(u"esp32bench-hello ready"_s), 30000);
            core.stopSim();
            QTRY_VERIFY(!core.isSimRunning());

            // Debug it: QEMU paused under the Arduino core's GDB, a breakpoint in
            // app_main, continue, and the stack where it stopped.
            const QString elf = QStringLiteral(ESP32BENCH_SOURCE_DIR
                                               "/../tests/firmware/build/hello-esp32/esp32bench-hello.elf");
            const QString gdb = QDir::homePath()
                + u"/.arduino15/packages/esp32/tools/xtensa-esp-elf-gdb/17.1_20260402/bin/xtensa-esp32-elf-gdb"_s;
            if (QFile::exists(elf) && QFile::exists(gdb)) {
                const QString built = dir.filePath(u"cache/esp32bench/builds/blink/esp32/out/blink.ino.elf"_s);
                QVERIFY(QFile::remove(built));
                QVERIFY(QFile::copy(elf, built));
                DebugPanel *dbg = window.debugger();
                window.buildPanel()->debugButton()->click();
                QTRY_VERIFY_WITH_TIMEOUT(dbg->statusText().contains(u"before the firmware starts"_s), 30000);
                dbg->locationInput()->setText(u"app_main"_s);
                dbg->addButton()->click();
                QTRY_COMPARE_WITH_TIMEOUT(dbg->breakpoints()->count(), 1, 10000);
                dbg->continueButton()->click();
                QTRY_VERIFY_WITH_TIMEOUT(dbg->statusText().contains(u"Stopped at breakpoint 1 in app_main (hello.c"_s), 30000);
                QTRY_VERIFY_WITH_TIMEOUT(dbg->stack()->topLevelItemCount() > 0, 10000);
                QCOMPARE(dbg->stack()->topLevelItem(0)->text(0), u"app_main"_s);
                QVERIFY(dbg->registers()->topLevelItemCount() > 0);
                core.stopSim();
                QTRY_VERIFY(!core.isSimRunning());
            } else {
                qWarning("hello ELF or GDB missing; debugger step skipped");
            }

            // Firmware that draws on QEMU's virtual RGB panel: the Display dock opens
            // with its picture (red, green, blue and white quadrants).
            const QString rgb = QStringLiteral(ESP32BENCH_SOURCE_DIR "/../tests/firmware/out/rgb-esp32.bin");
            if (QFile::exists(rgb)) {
                QVERIFY(QFile::remove(image));
                QVERIFY(QFile::copy(rgb, image));
                window.buildPanel()->simulateButton()->click();
                QTRY_VERIFY_WITH_TIMEOUT(window.display()->image().size() == QSize(320, 240)
                                             && window.display()->image().pixelColor(250, 200) == QColor(Qt::white),
                                         30000);
                QVERIFY(window.display()->isVisible());
                QCOMPARE(window.display()->image().pixelColor(250, 10), QColor(0, 255, 0));
                core.stopSim();
                QTRY_VERIFY(!core.isSimRunning());
            } else {
                qWarning("rgb firmware missing; display step skipped");
            }
        } else {
            qWarning("hello firmware or QEMU missing; simulator step skipped");
        }


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
