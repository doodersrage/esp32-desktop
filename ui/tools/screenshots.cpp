// Pictures of the app for the README and esp32bench.dev, taken from the real app
// against the real core: the library and a project, a simulation with its parts,
// and the debugger paused at a breakpoint. Run through ui/tools/screenshots.sh,
// which gives it its own settings and data folders and builds the projects first;
// the app never builds anything here, so no consent is skipped.

#include "BuildPanel.h"
#include "CircuitPanel.h"
#include "CoreConnection.h"
#include "DebugPanel.h"
#include "Icons.h"
#include "LibraryPanel.h"
#include "MainWindow.h"
#include "PinsPanel.h"
#include "ProjectView.h"
#include "SimulatorPanel.h"

#include <QAction>
#include <QApplication>
#include <QDeadlineTimer>
#include <QDir>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
#include <QPushButton>
#include <QTimer>
#include <QTreeWidget>

#include <cstdio>
#include <functional>

using namespace Qt::StringLiterals;

namespace {

// Let the app run until `done` holds, or fail after `ms`.
bool waitFor(const std::function<bool()> &done, int ms, const char *what)
{
    std::fprintf(stderr, "screenshots: waiting for %s\n", what);
    QDeadlineTimer deadline(ms);
    while (!done()) {
        // A dialog would wait for a person (a consent prompt, say): say so and stop.
        if (QWidget *modal = QApplication::activeModalWidget()) {
            std::fprintf(stderr, "screenshots: a dialog is open: %s\n", qPrintable(modal->windowTitle()));
            return false;
        }
        if (deadline.hasExpired()) {
            std::fprintf(stderr, "screenshots: timed out waiting for %s\n", what);
            return false;
        }
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
    }
    return true;
}

void settle(int ms)
{
    QDeadlineTimer deadline(ms);
    while (!deadline.hasExpired())
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
}

bool save(QWidget *window, const QString &dir, const QString &name)
{
    const QString path = dir + u"/"_s + name;
    if (!window->grab().save(path)) {
        std::fprintf(stderr, "screenshots: could not write %s\n", qPrintable(path));
        return false;
    }
    std::printf("%s\n", qPrintable(path));
    return true;
}

} // namespace

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(u"esp32bench"_s);
    QApplication::setOrganizationName(u"esp32bench"_s);
    Icons::install();
    if (argc < 2) {
        std::fprintf(stderr, "usage: esp32bench-screenshots <output folder>\n");
        return 2;
    }
    const QString out = QString::fromLocal8Bit(argv[1]);
    QDir().mkpath(out);

    // A dialog runs its own event loop, so waitFor cannot see one opened inside an
    // action: this timer, which runs in any loop, says which and stops.
    QTimer watchdog;
    QObject::connect(&watchdog, &QTimer::timeout, [] {
        QWidget *modal = QApplication::activeModalWidget();
        if (!modal)
            return;
        QString text;
        if (auto *box = qobject_cast<QMessageBox *>(modal))
            text = box->text();
        std::fprintf(stderr, "screenshots: a dialog opened, which needs a person: %s %s\n",
                     qPrintable(modal->windowTitle()), qPrintable(text));
        std::exit(1);
    });
    watchdog.start(2000);

    CoreConnection core;
    MainWindow window(&core);
    window.resize(1920, 1080);
    window.show();
    core.setAutoStart(true); // as the app does: start the core (ESP32BENCH_DAEMON)
    core.start();
    if (!waitFor([&] { return core.state() == CoreConnection::State::Connected; }, 20000, "the core"))
        return 1;
    if (!waitFor([&] { return window.libraryPanel()->tree()->topLevelItemCount() > 10; }, 60000, "the library"))
        return 1;

    // 1. The library and a project, as a person first sees them.
    window.openLink(u"esp32bench://project/oled?board=esp32"_s);
    if (!waitFor([&] { return window.windowTitle().contains(u"OLED"_s); }, 20000, "the OLED project"))
        return 1;
    if (!waitFor([&] { return window.projectView()->buildStateText().contains(u"ready to run"_s); }, 20000,
                 "OLED's build (ui/tools/screenshots.sh builds it)"))
        return 1;
    // Its wiring's parts on the canvas, from the Circuit dock's offer.
    CircuitPanel *circuit = window.circuit();
    if (!waitFor([&] { return circuit->addSuggestedButton()->isVisible(); }, 20000, "the suggested parts"))
        return 1;
    circuit->addSuggestedButton()->click();
    if (!waitFor([&] { return !circuit->parts().isEmpty() && !circuit->isSaving(); }, 20000, "the parts"))
        return 1;
    settle(1500);
    if (!save(&window, out, u"library.png"_s))
        return 1;

    // 2. The simulation: the console, the OLED drawing on the canvas, the pins.
    window.buildPanel()->simulateAction()->trigger();
    if (!waitFor([&] { return window.simulator()->outputText().contains(u"OLED"_s, Qt::CaseInsensitive)
                              || window.simulator()->outputText().count(u'\n') > 8; },
                 90000, "the simulation's output"))
        return 1;
    settle(6000); // a few frames on the OLED
    if (!save(&window, out, u"simulator.png"_s))
        return 1;
    core.stopSim();
    if (!waitFor([&] { return !core.isSimRunning(); }, 20000, "the simulation to stop"))
        return 1;

    // 3. The debugger, paused at a breakpoint in Blink's loop.
    window.openLink(u"esp32bench://project/blink?board=esp32"_s);
    if (!waitFor([&] { return window.windowTitle().contains(u"Blink"_s); }, 20000, "the Blink project"))
        return 1;
    if (!waitFor([&] { return window.projectView()->buildStateText().contains(u"ready to run"_s); }, 20000,
                 "Blink's build (ui/tools/screenshots.sh builds it)"))
        return 1;
    window.showLayout(MainWindow::Layout::Debugging);
    DebugPanel *debugger = window.debugger();
    // As a person does: Debug stops before the firmware starts; add a breakpoint
    // there, then Continue.
    window.buildPanel()->debugAction()->trigger();
    if (!waitFor([&] { return debugger->addButton()->isEnabled() || debugger->continueButton()->isEnabled(); },
                 120000, "the debugger to start"))
        return 1;
    debugger->locationInput()->setText(u"loop"_s);
    if (!waitFor([&] { return debugger->addButton()->isEnabled(); }, 10000, "Add breakpoint"))
        return 1;
    debugger->addButton()->click();
    if (!waitFor([&] { return !debugger->breakpointLocations().isEmpty(); }, 10000, "the breakpoint to be set"))
        return 1;
    debugger->continueButton()->click();
    if (!waitFor([&] { return debugger->stack()->topLevelItemCount() > 0
                              && debugger->stack()->topLevelItem(0)->text(0).contains(u"loop"_s); },
                 120000, "the breakpoint")) {
        std::fprintf(stderr, "screenshots: the debugger says: %s; breakpoints: %s; top frame: %s\n",
                     qPrintable(debugger->statusText()), qPrintable(debugger->breakpointLocations().join(u", "_s)),
                     debugger->stack()->topLevelItemCount()
                         ? qPrintable(debugger->stack()->topLevelItem(0)->text(0))
                         : "none");
        return 1;
    }
    debugger->addWatch(u"millis()"_s);
    settle(2000);
    if (!save(&window, out, u"debugger.png"_s))
        return 1;
    core.stopSim();
    waitFor([&] { return !core.isSimRunning(); }, 20000, "the debug run to stop");
    return 0;
}
