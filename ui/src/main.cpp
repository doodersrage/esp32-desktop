#include "CoreConnection.h"
#include "MainWindow.h"

#include <QApplication>

using namespace Qt::StringLiterals;

int main(int argc, char *argv[])
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(u"esp32bench"_s);
    QApplication::setOrganizationName(u"esp32bench"_s);
    QApplication::setApplicationVersion(QStringLiteral(ESP32BENCH_UI_VERSION));
    QApplication::setDesktopFileName(u"esp32bench"_s);

    CoreConnection core;
    MainWindow window(&core);
    window.show();
    core.start();
    return QApplication::exec();
}
