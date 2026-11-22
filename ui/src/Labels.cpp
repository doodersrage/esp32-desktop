#include "Labels.h"
#include "Translations.h"

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

QString Labels::forumPost(const QString &context, const QString &text)
{
    const QString app = QCoreApplication::translate("Labels", "ESP32 Bench %1").arg(QCoreApplication::applicationVersion());
    const QString about = context.isEmpty() ? app : QCoreApplication::translate("Labels", "%1 · %2").arg(app, context);
    // A fence the text cannot close.
    const QString fence = text.contains(QStringLiteral("```")) ? QStringLiteral("~~~~") : QStringLiteral("```");
    QString body = text;
    while (body.endsWith(QLatin1Char('\n')))
        body.chop(1);
    return QStringLiteral("%1\n%2text\n%3\n%2\n").arg(about, fence, body);
}

QString Labels::pinNoteText(const QString &kind, int gpio, const QString &why)
{
    // The site's reason for this board (in English), after the app's own words: in an
    // English app only, so a translated one stays in its language.
    const auto withWhy = [&](const QString &text) {
        return why.isEmpty() || Translations::translated()
                   ? text
                   : QCoreApplication::translate("Labels", "%1 On this board: %2").arg(text, why);
    };
    if (kind == u"strapping")
        return withWhy(QCoreApplication::translate("Labels", "GPIO %1 is a strapping pin: a part that pulls it high or low at "
                                                     "power-on can stop the board starting, or start it in another mode.")
            .arg(gpio));
    if (kind == u"inputOnly")
        return withWhy(QCoreApplication::translate("Labels", "GPIO %1 is input only: it cannot drive a part, and has no "
                                                     "internal pull-up or pull-down.")
            .arg(gpio));
    if (kind == u"flash")
        return withWhy(QCoreApplication::translate("Labels", "GPIO %1 is wired to the module's flash: using it crashes the chip.")
            .arg(gpio));
    if (kind == u"usb")
        return withWhy(QCoreApplication::translate("Labels", "GPIO %1 is one of the chip's USB pins: a board that talks over "
                                                     "USB needs it.")
            .arg(gpio));
    if (kind == u"console")
        return withWhy(QCoreApplication::translate("Labels", "GPIO %1 is the console's UART: what it prints and flashing use it.")
            .arg(gpio));
    if (kind == u"caution")
        return withWhy(QCoreApplication::translate("Labels", "GPIO %1 has another job on this board; its page on "
                                                             "esp32bench.dev says which.")
            .arg(gpio));
    return {};
}
