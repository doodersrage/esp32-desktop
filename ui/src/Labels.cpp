#include "Labels.h"

#include <QUrl>

using namespace Qt::StringLiterals;

QString Labels::chipName(const QString &id)
{
    // Product names; not translated.
    if (id == u"esp32") return u"ESP32"_s;
    if (id == u"esp32s2") return u"ESP32-S2"_s;
    if (id == u"esp32s3") return u"ESP32-S3"_s;
    if (id == u"esp32c3") return u"ESP32-C3"_s;
    if (id == u"esp32c6") return u"ESP32-C6"_s;
    return id;
}

QString Labels::projectLink(const QString &id, const QString &board)
{
    QUrl url;
    url.setScheme(QStringLiteral("esp32bench"));
    url.setHost(QStringLiteral("project"));
    url.setPath(QLatin1Char('/') + id);
    if (!board.isEmpty())
        url.setQuery(QStringLiteral("board=") + QString::fromLatin1(QUrl::toPercentEncoding(board)));
    return url.toString(QUrl::FullyEncoded);
}
