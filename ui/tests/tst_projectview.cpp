// ProjectView renders library.get results. The fixture is the example in
// schema/rpc/library.get.json, the same data the core's schema test checks.

#include "CoreConnection.h"
#include "ProjectView.h"

#include <QComboBox>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTabWidget>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

class TestProjectView : public QObject {
    Q_OBJECT

private:
    static QJsonObject example()
    {
        QFile f(QStringLiteral(ESP32BENCH_SOURCE_DIR "/../schema/rpc/library.get.json"));
        if (!f.open(QIODevice::ReadOnly))
            qFatal("cannot open %s", qPrintable(f.fileName()));
        const QJsonObject def = QJsonDocument::fromJson(f.readAll()).object();
        return def.value(u"examples"_s).toArray().at(0).toObject().value(u"result"_s).toObject();
    }

private slots:
    void rendersProjectAndSwitchesBoards()
    {
        CoreConnection core;
        ProjectView view(&core);
        const QJsonObject result = example();
        const QJsonObject p = result.value(u"project"_s).toObject();
        view.showResult(result);

        QCOMPARE(view.title(), p.value(u"name"_s).toString());
        const QJsonArray builds = p.value(u"builds"_s).toArray();
        QCOMPARE(view.boardPicker()->count(), builds.size());
        QVERIFY(builds.size() >= 2);
        // Board names come from the result's boards, with the chip.
        QVERIFY(view.boardPicker()->itemText(0).contains(u"(ESP32"_s));
        QCOMPARE(view.partsTree()->topLevelItemCount(), p.value(u"parts"_s).toArray().size());

        const QString first = builds.at(0).toObject().value(u"board"_s).toString();
        const QString second = builds.at(1).toObject().value(u"board"_s).toString();
        QVERIFY(view.codeView()->toPlainText().contains(first));
        QCOMPARE(view.wiringTree()->topLevelItemCount(),
                 builds.at(0).toObject().value(u"wiring"_s).toArray().size());
        view.boardPicker()->setCurrentIndex(1);
        QVERIFY(view.codeView()->toPlainText().contains(second));
    }

    void saysWhenFirmwareIsPrebuilt()
    {
        CoreConnection core;
        ProjectView view(&core);
        QJsonObject result = example();
        QJsonObject p = result.value(u"project"_s).toObject();
        p[u"firmware"_s] = u"prebuilt"_s;
        QJsonArray builds;
        for (const QJsonValue &v : p.value(u"builds"_s).toArray()) {
            QJsonObject b = v.toObject();
            b.remove(u"code"_s);
            builds.append(b);
        }
        p[u"builds"_s] = builds;
        result[u"project"_s] = p;
        view.showResult(result);
        QVERIFY(view.codeView()->toPlainText().contains(u"prebuilt firmware"_s));
    }

    void escapesNotesAndIgnoresStaleReplies()
    {
        CoreConnection core;
        ProjectView view(&core);
        QJsonObject result = example();
        QJsonObject p = result.value(u"project"_s).toObject();
        p[u"notes"_s] = QJsonArray{u"Use <b>3V3</b>, not 5 V & never 12 V."_s};
        result[u"project"_s] = p;
        view.showResult(result);
        QVERIFY(view.notesText().contains(u"&lt;b&gt;3V3&lt;/b&gt;"_s));
        QVERIFY(view.notesText().contains(u"&amp; never"_s));

        // The user has since picked another project; a late reply for this one is ignored.
        const QString shown = view.title();
        view.showProject(u"something-else"_s);
        QJsonObject other = example();
        QJsonObject op = other.value(u"project"_s).toObject();
        op[u"name"_s] = u"Late reply"_s;
        other[u"project"_s] = op;
        view.showResult(other);
        QVERIFY(view.title() != u"Late reply"_s);
        QCOMPARE(view.title(), shown);
    }

    // A local ESP-IDF project (local.get): the chips to build for, the ESP-IDF that
    // builds it, a folder button instead of the site, and no library tabs.
    void showsALocalProject()
    {
        QFile f(QStringLiteral(ESP32BENCH_SOURCE_DIR "/../schema/rpc/local.get.json"));
        QVERIFY(f.open(QIODevice::ReadOnly));
        QJsonObject result = QJsonDocument::fromJson(f.readAll())
                                 .object()
                                 .value(u"examples"_s)
                                 .toArray()
                                 .at(0)
                                 .toObject()
                                 .value(u"result"_s)
                                 .toObject();
        CoreConnection core;
        ProjectView view(&core);
        QList<QStringList> selected;
        connect(&view, &ProjectView::boardSelected, this,
                [&](const QString &project, const QString &board, const QJsonArray &) { selected << QStringList{project, board}; });
        view.showLocal(result);
        QCOMPARE(view.title(), u"blinky"_s);
        QVERIFY(view.summaryText().contains(u"ESP-IDF 6.0.2"_s));
        QCOMPARE(view.boardPicker()->count(), 3);
        // Chips, not boards: an ESP-IDF build runs on any board with the chip.
        QCOMPARE(view.boardPicker()->itemText(1), u"ESP32-S3"_s);
        QVERIFY(view.tabs()->isHidden());
        QCOMPARE(view.openButton()->text(), u"Open folder"_s);
        QVERIFY(!view.buildButton()->isHidden());
        QCOMPARE(selected.last(), (QStringList{u"local-3f9a0c21b4de"_s, u"esp32"_s}));
        view.boardPicker()->setCurrentIndex(2);
        QCOMPARE(selected.last(), (QStringList{u"local-3f9a0c21b4de"_s, u"esp32c3"_s}));

        // No ESP-IDF, or the folder gone: it says so.
        QJsonObject noIdf = result;
        noIdf.insert(u"idf"_s, QJsonValue::Null);
        view.showLocal(noIdf);
        QVERIFY(view.summaryText().contains(u"ESP-IDF was not found"_s));
        QJsonObject gone = result;
        gone.insert(u"exists"_s, false);
        view.showLocal(gone);
        QVERIFY(view.summaryText().contains(u"no longer in"_s));

        // Back to a library project: its tabs and site button return.
        view.showResult(example());
        QVERIFY(!view.tabs()->isHidden());
        QCOMPARE(view.openButton()->text(), u"Open on esp32bench.dev"_s);
    }
};

QTEST_MAIN(TestProjectView)
#include "tst_projectview.moc"
