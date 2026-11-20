#include "Icons.h"

#include <QIcon>
#include <QStringList>

using namespace Qt::StringLiterals;

void Icons::install()
{
    QStringList paths = QIcon::themeSearchPaths();
    if (!paths.contains(u":/icons"_s))
        paths << u":/icons"_s;
    QIcon::setThemeSearchPaths(paths);
    QIcon::setFallbackThemeName(QString::fromLatin1(FallbackTheme));
}
