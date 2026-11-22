// The UI's translations: each language is built into the app and loads, and every
// .ts file is complete and keeps each string's placeholders and accelerators.

#include "Translations.h"

#include <QCoreApplication>
#include <QFile>
#include <QRegularExpression>
#include <QTest>
#include <QTranslator>
#include <QXmlStreamReader>

using namespace Qt::StringLiterals;

namespace {

struct Message {
    QString context, source, translation;
    bool finished = false;
};

QList<Message> readTs(const QString &path)
{
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly))
        return {};
    QList<Message> out;
    QXmlStreamReader xml(&f);
    QString context;
    Message m;
    while (!xml.atEnd()) {
        xml.readNext();
        if (!xml.isStartElement())
            continue;
        if (xml.name() == "name"_L1)
            context = xml.readElementText();
        else if (xml.name() == "message"_L1)
            m = Message{context, {}, {}, false};
        else if (xml.name() == "source"_L1)
            m.source = xml.readElementText();
        else if (xml.name() == "translation"_L1) {
            const QString type = xml.attributes().value("type"_L1).toString();
            // A plural (%n) has one <numerusform> per form; each is checked as a
            // translation of its own.
            QStringList forms;
            QString text;
            while (!xml.atEnd()) {
                xml.readNext();
                if (xml.isStartElement() && xml.name() == "numerusform"_L1)
                    forms << xml.readElementText();
                else if (xml.isCharacters())
                    text += xml.text();
                else if (xml.isEndElement() && xml.name() == "translation"_L1)
                    break;
            }
            if (forms.isEmpty())
                forms << text;
            for (const QString &form : std::as_const(forms)) {
                m.translation = form;
                m.finished = type.isEmpty() && !form.trimmed().isEmpty();
                out.append(m);
            }
        }
    }
    return xml.hasError() ? QList<Message>{} : out;
}

// %1, %L2, %n...: sorted, so order may change between languages.
QStringList placeholders(const QString &s)
{
    static const QRegularExpression re(u"%L?(\\d+|n)"_s);
    QStringList out;
    for (auto it = re.globalMatch(s); it.hasNext();)
        out.append(it.next().captured(0));
    out.sort();
    return out;
}

// Accelerator markers: "&" before a character other than another "&".
int accelerators(const QString &s)
{
    int n = 0;
    for (qsizetype i = 0; i + 1 < s.size(); ++i) {
        if (s[i] != u'&')
            continue;
        if (s[i + 1] == u'&') {
            ++i;
            continue;
        }
        if (!s[i + 1].isSpace())
            ++n;
    }
    return n;
}

QString tsPath(const QString &lang)
{
    return QStringLiteral(ESP32BENCH_SOURCE_DIR) + u"/translations/esp32bench_"_s + lang + u".ts"_s;
}

} // namespace

class TestTranslations : public QObject
{
    Q_OBJECT
private slots:
    void filesAreComplete_data()
    {
        QTest::addColumn<QString>("lang");
        for (const QString &l : Translations::languages())
            QTest::newRow(qPrintable(l)) << l;
    }
    void filesAreComplete()
    {
        QFETCH(QString, lang);
        const QList<Message> msgs = readTs(tsPath(lang));
        QVERIFY2(!msgs.isEmpty(), "could not read the .ts file");
        // Every language has the same strings as the first.
        const QList<Message> ref = readTs(tsPath(Translations::languages().first()));
        QCOMPARE(msgs.size(), ref.size());
        QStringList problems;
        for (const Message &m : msgs) {
            const QString where = m.context + u": "_s + m.source;
            if (!m.finished)
                problems << u"unfinished: "_s + where;
            else if (placeholders(m.source) != placeholders(m.translation))
                problems << u"placeholders differ: "_s + where;
            else if (accelerators(m.source) != accelerators(m.translation))
                problems << u"accelerators differ: "_s + where;
        }
        QVERIFY2(problems.isEmpty(), qPrintable(problems.mid(0, 20).join(u'\n')));
    }

    void builtIntoTheApp_data() { filesAreComplete_data(); }
    void builtIntoTheApp()
    {
        QFETCH(QString, lang);
        QTranslator t;
        QVERIFY(t.load(QLocale(lang), u"esp32bench"_s, u"_"_s, u":/i18n"_s));
        const QString file = t.translate("MainWindow", "&File");
        QVERIFY(!file.isEmpty());
        QVERIFY(file != u"&File"_s);
    }

    void installForTheLocale()
    {
        QCoreApplication *app = QCoreApplication::instance();
        QVERIFY(app);
        QVERIFY(!Translations::install(*app, QLocale(u"en_US"_s)));
        QVERIFY(!Translations::install(*app, QLocale(u"ja_JP"_s)));
        QVERIFY(Translations::install(*app, QLocale(u"pt_BR"_s)));
        QCOMPARE(QCoreApplication::translate("MainWindow", "&File").isEmpty(), false);
        QVERIFY(QCoreApplication::translate("MainWindow", "&File") != u"&File"_s);
    }
};

QTEST_GUILESS_MAIN(TestTranslations)
#include "tst_translations.moc"
