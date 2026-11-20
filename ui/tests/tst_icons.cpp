// Every icon the UI asks for has one in the bundled fallback theme, which macOS and
// Windows use (they have no icon theme of their own).

#include "Icons.h"

#include <QDirIterator>
#include <QFile>
#include <QIcon>
#include <QImage>
#include <QPainter>
#include <QRegularExpression>
#include <QTest>

using namespace Qt::StringLiterals;

class TestIcons : public QObject {
    Q_OBJECT

private slots:
    void fallbackThemeHasEveryIconTheUiUses()
    {
        Icons::install();
        // As on a system with no theme: only the fallback answers.
        QIcon::setThemeName(QString::fromLatin1(Icons::FallbackTheme));
        const QRegularExpression name(uR"re(fromTheme\(u"([a-z0-9-]+)"_s)re"_s);
        QStringList used;
        QDirIterator it(QStringLiteral(ESP32BENCH_SOURCE_DIR "/src"), {u"*.cpp"_s});
        while (it.hasNext()) {
            QFile f(it.next());
            QVERIFY(f.open(QIODevice::ReadOnly));
            const QString text = QString::fromUtf8(f.readAll());
            for (auto m = name.globalMatch(text); m.hasNext();) {
                const QString n = m.next().captured(1);
                if (n != u"esp32bench" && !used.contains(n)) // the app's own icon is bundled apart
                    used << n;
            }
        }
        QVERIFY(used.size() > 20);
        QStringList missing;
        for (const QString &n : std::as_const(used)) {
            if (QIcon::fromTheme(n).isNull())
                missing << n;
        }
        QVERIFY2(missing.isEmpty(), qPrintable(u"no fallback icon for: "_s + missing.join(u", "_s)));
        // A name with no icon gets none, so the check above can fail.
        QVERIFY(QIcon::fromTheme(u"no-such-icon-anywhere"_s).isNull());
        // And they draw. ESP32BENCH_SCREENSHOT saves them on light and dark, to look at.
        QVERIFY(!QIcon::fromTheme(u"media-playback-start"_s).pixmap(24, 24).isNull());
        if (const QString shot = qEnvironmentVariable("ESP32BENCH_SCREENSHOT"); !shot.isEmpty()) {
            QImage sheet(used.size() * 28, 56, QImage::Format_ARGB32);
            sheet.fill(Qt::white);
            QPainter p(&sheet);
            p.fillRect(0, 28, sheet.width(), 28, QColor(0x23, 0x26, 0x29));
            for (int i = 0; i < used.size(); ++i) {
                const QPixmap pm = QIcon::fromTheme(used.at(i)).pixmap(24, 24);
                p.drawPixmap(i * 28 + 2, 2, pm);
                p.drawPixmap(i * 28 + 2, 30, pm);
            }
            p.end();
            QVERIFY(sheet.save(shot));
        }
    }
};

QTEST_MAIN(TestIcons)
#include "tst_icons.moc"
