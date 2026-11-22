#include "Labels.h"

#include <QCoreApplication>
#include <QLocale>
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

QString Labels::ago(const QDateTime &when, const QDateTime &now)
{
    const qint64 secs = when.secsTo(now);
    if (secs < 60)
        return QCoreApplication::translate("Labels", "just now");
    if (secs < 3600)
        return QCoreApplication::translate("Labels", "%1 min ago").arg(secs / 60);
    if (secs < 12 * 3600)
        return QCoreApplication::translate("Labels", "%1 h ago").arg(secs / 3600);
    return QLocale().toString(when, QLocale::ShortFormat);
}

QString Labels::duration(double seconds)
{
    const qint64 tenths = qRound64(seconds * 10);
    if (tenths < 600)
        return QCoreApplication::translate("Labels", "%1 s").arg(QLocale().toString(tenths / 10.0, 'f', 1));
    const qint64 whole = tenths / 10;
    return QCoreApplication::translate("Labels", "%1 min %2 s").arg(whole / 60).arg(whole % 60, 2, 10, QLatin1Char('0'));
}
