#include "CoreConnection.h"
#include "MainWindow.h"
#include "SerialMonitor.h"
#include "Icons.h"
#include "Preferences.h"
#include "Translations.h"

#include <QApplication>
#include <QCommandLineParser>
#include <QDir>
#include <QIcon>
#include <QLocalServer>
#include <QLocalSocket>
#include <QStandardPaths>

using namespace Qt::StringLiterals;

namespace {

// One window per user session. A second launch (a link clicked while the app runs)
// hands its link to the first through this socket and exits. The socket lives in
// the user's runtime directory, so only this user can reach it.
QString instanceSocket()
{
    const QString dir = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation) + u"/esp32bench"_s;
    QDir().mkpath(dir);
    return dir + u"/gui.sock"_s;
}

// Pass `link` (empty: just raise the window) to a running instance. True if one
// took it.
bool handToRunningInstance(const QString &link)
{
    QLocalSocket socket;
    socket.connectToServer(instanceSocket());
    if (!socket.waitForConnected(500))
        return false;
    socket.write(link.toUtf8() + '\n');
    socket.flush();
    socket.waitForBytesWritten(1000);
    socket.disconnectFromServer();
    return true;
}

} // namespace

int main(int argc, char *argv[])
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(u"esp32bench"_s);
    QApplication::setOrganizationName(u"esp32bench"_s);
    QApplication::setApplicationVersion(QStringLiteral(ESP32BENCH_UI_VERSION));
    QApplication::setDesktopFileName(u"esp32bench"_s);
    QApplication::setWindowIcon(QIcon::fromTheme(u"esp32bench"_s, QIcon(u":/esp32bench.svg"_s)));
    // The language chosen in Preferences, else the desktop's.
    const QString language = Preferences::language();
    if (language != u"en")
        Translations::install(app, language.isEmpty() ? QLocale() : QLocale(language));
    Icons::install();

    QCommandLineParser parser;
    parser.setApplicationDescription(QApplication::translate("main", "ESP32 workbench and simulator"));
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addPositionalArgument(u"link"_s,
                                 QApplication::translate("main", "An esp32bench:// link to open, from esp32bench.dev"),
                                 u"[link]"_s);
    parser.process(app);
    // The core checks the link; here it is only passed along.
    const QString link = parser.positionalArguments().value(0);

    if (handToRunningInstance(link))
        return 0;

    CoreConnection core;
    MainWindow window(&core);
    window.serialMonitor()->restoreLastPort();
    window.sayWhatsNew();

    QLocalServer server;
    QLocalServer::removeServer(instanceSocket()); // left behind by a crash
    server.setSocketOptions(QLocalServer::UserAccessOption);
    if (server.listen(instanceSocket())) {
        QObject::connect(&server, &QLocalServer::newConnection, &window, [&server, &window] {
            QLocalSocket *s = server.nextPendingConnection();
            QObject::connect(s, &QLocalSocket::disconnected, s, &QObject::deleteLater);
            QObject::connect(s, &QLocalSocket::readyRead, &window, [s, &window] {
                while (s->canReadLine()) {
                    const QByteArray line = s->readLine(512).trimmed();
                    window.openLink(QString::fromUtf8(line));
                }
            });
        });
    }

    window.show();
    core.setAutoStart(true);
    core.start();
    if (!link.isEmpty())
        window.openLink(link);
    return QApplication::exec();
}
