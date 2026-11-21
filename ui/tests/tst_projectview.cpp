// ProjectView renders library.get results. The fixture is the example in
// schema/rpc/library.get.json, the same data the core's schema test checks.

#include "CoreConnection.h"
#include "ProjectView.h"

#include <QAction>
#include <QComboBox>
#include <QSignalSpy>
#include <QFile>
#include <QJsonArray>
#include <QLabel>
#include <QJsonDocument>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTabWidget>
#include <QMenu>
#include <QToolButton>
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
        // Before a project: nothing for the toolbar, and nothing to build.
        QVERIFY(view.currentProjectTitle().isEmpty());
        QVERIFY(!view.buildAction()->isEnabled());
        QSignalSpy changed(&view, &ProjectView::currentChanged);
        const QJsonObject result = example();
        const QJsonObject p = result.value(u"project"_s).toObject();
        view.showResult(result);

        QCOMPARE(view.title(), p.value(u"name"_s).toString());
        QVERIFY(changed.size() >= 1);
        QCOMPARE(view.currentProjectTitle(), view.title());
        QCOMPARE(view.currentBoardName(), view.boardPicker()->currentText());
        // Not connected to a core, so the button and the toolbar's action agree: off.
        QCOMPARE(view.buildAction()->isEnabled(), view.buildButton()->isEnabled());
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

    void startsOnTheConnectedBoardsChip()
    {
        // The example, with an ESP32-S3 build added after the ESP32 ones.
        QJsonObject result = example();
        QJsonObject p = result.value(u"project"_s).toObject();
        QJsonArray builds = p.value(u"builds"_s).toArray();
        QJsonObject s3 = builds.at(0).toObject();
        s3[u"board"_s] = u"esp32s3"_s;
        builds.append(s3);
        p[u"builds"_s] = builds;
        result[u"project"_s] = p;
        QJsonArray boards = result.value(u"boards"_s).toArray();
        boards.append(QJsonObject{{u"id"_s, u"esp32s3"_s}, {u"name"_s, u"ESP32-S3 DevKitC-1"_s},
                                  {u"short"_s, u"S3"_s}, {u"chip"_s, u"esp32s3"_s}, {u"arduinoBoard"_s, u"x"_s}});
        result[u"boards"_s] = boards;

        CoreConnection core;
        ProjectView view(&core);
        view.setConnectedChips({u"esp32s3"_s});
        view.showResult(result);
        QCOMPARE(view.boardPicker()->currentData().toString(), u"esp32s3"_s);

        // Plugged in while shown: it switches, unless the board was chosen by hand.
        ProjectView later(&core);
        later.showResult(result);
        QCOMPARE(later.boardPicker()->currentIndex(), 0);
        later.setConnectedChips({u"esp32s3"_s});
        QCOMPARE(later.boardPicker()->currentData().toString(), u"esp32s3"_s);

        ProjectView chosen(&core);
        chosen.showResult(result);
        emit chosen.boardPicker()->activated(0); // as a click on the first board does
        chosen.setConnectedChips({u"esp32s3"_s});
        QCOMPARE(chosen.boardPicker()->currentIndex(), 0);
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
        // No code to copy or save.
        QVERIFY(view.copyCodeButton()->isHidden());
        QVERIFY(view.saveCodeButton()->isHidden());
        // No image linked: nothing to build or download.
        QVERIFY(view.buildButton()->isHidden());

        // With the library's image (manifest 1.1) it is downloaded instead of built.
        QJsonArray withImage;
        for (const QJsonValue &v : builds) {
            QJsonObject b = v.toObject();
            b[u"image"_s] = QJsonObject{{u"url"_s, u"https://esp32bench.dev/firmware/x.bin"_s},
                                        {u"sha256"_s, QString(64, u'a')}, {u"size"_s, 1000}};
            withImage.append(b);
        }
        p[u"builds"_s] = withImage;
        result[u"project"_s] = p;
        view.showResult(result);
        QVERIFY(!view.buildButton()->isHidden());
        QCOMPARE(view.buildButton()->text(), u"Download"_s);
        QCOMPARE(view.buildAction()->text(), u"Download"_s);
    }

    void saysWhetherItIsBuilt()
    {
        CoreConnection core;
        ProjectView view(&core);
        view.showResult(example());
        QVERIFY(!view.copyCodeButton()->isHidden());
        QVERIFY(!view.saveCodeButton()->isHidden());
        QSignalSpy current(&view, &ProjectView::buildCurrent);
        view.showBuildState(QJsonObject{{u"built"_s, false}});
        QVERIFY(view.buildStateText().contains(u"Not built"_s));
        QCOMPARE(current.last().at(2).toBool(), false);
        view.showBuildState(QJsonObject{{u"built"_s, true}, {u"builtAt"_s, 1795000000}, {u"current"_s, true}});
        QVERIFY(view.buildStateText().contains(u"ready to run"_s));
        QCOMPARE(current.last().at(2).toBool(), true);
        view.showBuildState(QJsonObject{{u"built"_s, true}, {u"builtAt"_s, 1795000000}, {u"current"_s, false}});
        QVERIFY(view.buildStateText().contains(u"built again"_s));
        QCOMPARE(current.last().at(2).toBool(), false);
    }

    void listsRecentProjectsOnTheGuide()
    {
        CoreConnection core;
        ProjectView view(&core);
        QSignalSpy asked(&view, &ProjectView::recentRequested);
        view.showGuide();
        QVERIFY(view.recentHtml().isEmpty());
        view.setRecent({{u"Simon <says>"_s, u"simon"_s}, {u"Blink"_s, u"blink"_s}});
        QVERIFY(view.recentHtml().contains(u"href=\"project:simon\""_s));
        QVERIFY(view.recentHtml().contains(u"Simon &lt;says&gt;"_s));
        view.placeholderLabel()->linkActivated(u"project:blink"_s);
        QCOMPARE(asked.size(), 1);
        QCOMPARE(asked.at(0).at(0).toString(), u"blink"_s);
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
        // Its source files, from local.get, to open in the user's editor.
        QVERIFY(!view.editButton()->isHidden());
        QStringList files;
        for (QAction *a : view.editButton()->menu()->actions())
            files << a->text();
        QCOMPARE(files, (QStringList{u"CMakeLists.txt"_s, u"main/CMakeLists.txt"_s, u"main/blinky.c"_s}));
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
