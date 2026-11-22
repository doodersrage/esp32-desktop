// ProjectView renders library.get results. The fixture is the example in
// schema/rpc/library.get.json, the same data the core's schema test checks.

#include "CodeView.h"
#include "CoreConnection.h"
#include "Labels.h"
#include "SketchHighlighter.h"
#include "ProjectView.h"

#include <QAction>
#include <QComboBox>
#include <QDateTime>
#include <QLineEdit>
#include <QLocale>
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
#include <QClipboard>
#include <QGuiApplication>
#include <QApplication>
#include <QHelpEvent>
#include <QTextBlock>
#include <QTextLayout>
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
        // Ctrl+] and Ctrl+[: the next and previous board, round from the end.
        const int at = view.boardPicker()->currentIndex();
        QVERIFY(view.stepBoard(1));
        QCOMPARE(view.boardPicker()->currentIndex(), (at + 1) % int(builds.size()));
        QVERIFY(view.stepBoard(-1));
        QCOMPARE(view.boardPicker()->currentIndex(), at);
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

    void showsTheLineABuildErrorPointsAt()
    {
        CoreConnection core;
        ProjectView view(&core);
        QVERIFY(!view.showCodeLine(1)); // nothing shown
        view.showResult(example());
        view.tabs()->setCurrentIndex(ProjectView::PartsTab);
        const QString code = view.codeView()->toPlainText();
        const int lines = int(code.count(u'\n')) + 1;
        QVERIFY(lines >= 2);
        QVERIFY(view.showCodeLine(2));
        QCOMPARE(view.tabs()->currentIndex(), int(ProjectView::CodeTab));
        QCOMPARE(view.codeView()->textCursor().blockNumber(), 1);
        QCOMPARE(view.codeView()->textCursor().selectedText(), code.section(u'\n', 1, 1));
        QVERIFY(!view.showCodeLine(lines + 5));
        QVERIFY(!view.showCodeLine(0));
    }

    void codeGutterShowsBreakpointsAndTakesClicks()
    {
        CoreConnection core;
        ProjectView view(&core);
        view.resize(600, 500);
        view.show();
        QVERIFY(QTest::qWaitForWindowExposed(&view));
        view.showResult(example());
        view.tabs()->setCurrentIndex(ProjectView::CodeTab);
        CodeView *code = view.codeView();
        QVERIFY(code->gutterWidth() > 0);
        QSignalSpy clicked(&view, &ProjectView::codeLineClicked);
        // A click beside the second line asks for it.
        QWidget *gutter = code->findChild<QWidget *>(u"gutter"_s);
        QVERIFY(gutter);
        const int lineHeight = code->fontMetrics().height();
        int y = -1;
        for (int at = 0; at < lineHeight * 4 && y < 0; ++at)
            if (code->lineAt(at) == 2)
                y = at;
        QVERIFY(y >= 0);
        QTest::mouseClick(gutter, Qt::LeftButton, {}, QPoint(4, y + 1));
        QCOMPARE(clicked.size(), 1);
        QCOMPARE(clicked.at(0).at(0).toInt(), 2);
        code->setBreakpointLines({2});
        QCOMPARE(code->breakpointLines(), QSet<int>{2});
        // The paused line: marked, and the Code tab shown.
        view.tabs()->setCurrentIndex(ProjectView::PartsTab);
        view.showPausedLine(2);
        QCOMPARE(view.tabs()->currentIndex(), int(ProjectView::CodeTab));
        QCOMPARE(code->pausedLine(), 2);
        QCOMPARE(code->extraSelections().size(), 1);
        view.showPausedLine(0);
        QVERIFY(code->extraSelections().isEmpty());
    }

    void findsInTheCodeAndGoesToALine()
    {
        CoreConnection core;
        ProjectView view(&core);
        view.resize(600, 500);
        view.show();
        QVERIFY(QTest::qWaitForWindowExposed(&view));
        view.showResult(example());
        CodeView *code = view.codeView();
        const QString text = code->toPlainText();
        const QString second = text.section(u'\n', 1, 1).trimmed();
        QVERIFY(!second.isEmpty());

        QVERIFY(!view.codeFindField()->isVisible());
        view.findInCode();
        QCOMPARE(view.tabs()->currentIndex(), int(ProjectView::CodeTab));
        QVERIFY(view.codeFindField()->isVisible());
        QVERIFY(view.codeHasFocus());
        QTest::keyClicks(view.codeFindField(), second);
        QCOMPARE(code->textCursor().selectedText(), second);
        QCOMPARE(code->textCursor().blockNumber(), 1);
        QVERIFY(!code->findText(u"no such text anywhere"_s));
        QTest::keyClick(view.codeFindField(), Qt::Key_Escape);
        QVERIFY(!view.codeFindField()->isVisible());

        QVERIFY(code->goToLine(2));
        QCOMPARE(code->textCursor().blockNumber(), 1);
        QVERIFY(!code->goToLine(0));
        QVERIFY(!code->goToLine(int(text.count(u'\n')) + 5));

        // The line menu: a breakpoint, and Run to the line while paused.
        QSignalSpy runTo(&view, &ProjectView::runToLineRequested);
        QSignalSpy toggled(&view, &ProjectView::codeLineClicked);
        QMenu *menu = code->lineMenu(3);
        QCOMPARE(menu->actions().size(), 3);
        QVERIFY(!menu->actions().at(2)->isEnabled());
        QSignalSpy condition(code, &CodeView::conditionRequested);
        menu->actions().at(1)->trigger();
        QCOMPARE(condition.size(), 1);
        QCOMPARE(condition.at(0).at(0).toInt(), 3);
        menu->actions().at(0)->trigger();
        QCOMPARE(toggled.size(), 1);
        QCOMPARE(toggled.at(0).at(0).toInt(), 3);
        delete menu;
        code->setRunToEnabled(true);
        menu = code->lineMenu(3);
        QVERIFY(menu->actions().at(2)->isEnabled());
        menu->actions().at(2)->trigger();
        QCOMPARE(runTo.size(), 1);
        QCOMPARE(runTo.at(0).at(0).toInt(), 3);
        delete menu;
    }

    void coloursTheSketchAndNamesWhatIsHovered()
    {
        CodeView code;
        code.resize(500, 300);
        code.setPlainText(u"#include <Wire.h>\nint count = 42; // the count\nvoid loop() { Serial.println(\"hi /* not */\"); }\n/* a\nlong */ int x;"_s);
        code.show();
        QVERIFY(QTest::qWaitForWindowExposed(&code));
        const auto formatAt = [&](int block, int column) {
            const QTextBlock b = code.document()->findBlockByNumber(block);
            for (const QTextLayout::FormatRange &r : b.layout()->formats())
                if (column >= r.start && column < r.start + r.length)
                    return r.format;
            return QTextCharFormat();
        };
        SketchHighlighter *h = code.highlighter();
        QCOMPARE(formatAt(0, 2).foreground(), h->format(SketchHighlighter::Preprocessor).foreground());
        QCOMPARE(formatAt(1, 0).fontWeight(), int(QFont::DemiBold)); // int
        QCOMPARE(formatAt(1, 12).foreground(), h->format(SketchHighlighter::Number).foreground()); // 42
        QVERIFY(formatAt(1, 20).fontItalic()); // the comment
        // A comment marker inside a string is the string's.
        const int quote = int(code.document()->findBlockByNumber(2).text().indexOf(u"/*"_s));
        QCOMPARE(formatAt(2, quote).foreground(), h->format(SketchHighlighter::String).foreground());
        QVERIFY(formatAt(3, 1).fontItalic());
        QVERIFY(formatAt(4, 2).fontItalic()); // still in the comment
        QVERIFY(!formatAt(4, 9).fontItalic()); // int x, after it

        // The name under the mouse: hovering asks for its value only while paused.
        QTextCursor c(code.document()->findBlockByNumber(1));
        c.setPosition(c.position() + 6); // inside "count"
        const QPoint at = code.cursorRect(c).center();
        QCOMPARE(code.wordAt(at), u"count"_s);
        QSignalSpy hovered(&code, &CodeView::wordHovered);
        QHelpEvent tip(QEvent::ToolTip, at, code.viewport()->mapToGlobal(at));
        QApplication::sendEvent(code.viewport(), &tip);
        QCOMPARE(hovered.size(), 0);
        code.setRunToEnabled(true);
        QApplication::sendEvent(code.viewport(), &tip);
        QCOMPARE(hovered.size(), 1);
        QCOMPARE(hovered.at(0).at(0).toString(), u"count"_s);
        // Not a number.
        c.setPosition(code.document()->findBlockByNumber(1).position() + 13);
        QVERIFY(code.wordAt(code.cursorRect(c).center()).isEmpty());
    }

    void marksBuildProblemsAndBreaksOnAFunction()
    {
        CodeView code;
        code.resize(500, 300);
        code.setPlainText(u"void setup() {\n  foo(1);\n}\nvoid loop() {}\n"_s);
        code.show();
        QVERIFY(QTest::qWaitForWindowExposed(&code));
        code.setProblems({{2, {u"error"_s, u"blink.ino:2:3: error: 'foo' was not declared in this scope"_s}},
                          {4, {u"warning"_s, u"blink.ino:4: warning: unused"_s}}});
        QCOMPARE(code.extraSelections().size(), 2);
        QCOMPARE(code.extraSelections().at(0).format.underlineStyle(), QTextCharFormat::WaveUnderline);
        QVERIFY(code.problemAt(2).contains(u"was not declared"_s));
        QVERIFY(code.problemAt(1).isEmpty());
        // With the paused line too.
        code.setPausedLine(1);
        QCOMPARE(code.extraSelections().size(), 3);
        code.setProblems({});
        QCOMPARE(code.extraSelections().size(), 1);
        code.setPausedLine(0);

        // Right-click on a name: break when it runs.
        QTextCursor c(code.document()->findBlockByNumber(3));
        c.setPosition(c.position() + 6); // in "loop"
        QMenu *menu = code.menuAt(code.cursorRect(c).center());
        QAction *breakOn = nullptr;
        for (QAction *a : menu->actions())
            if (a->text() == u"Break when loop runs"_s)
                breakOn = a;
        QVERIFY(breakOn);
        QSignalSpy asked(&code, &CodeView::functionBreakRequested);
        breakOn->trigger();
        QCOMPARE(asked.size(), 1);
        QCOMPARE(asked.at(0).at(0).toString(), u"loop"_s);
        delete menu;
    }

    void hasMyNotesAndThePinout()
    {
        CoreConnection core;
        ProjectView view(&core);
        view.showResult(example());
        QCOMPARE(view.tabs()->widget(ProjectView::MyNotesTab), view.myNotes());
        QVERIFY(!view.pinoutButton()->isHidden());
        QCOMPARE(ProjectView::pinoutUrl(u"esp32c3-supermini"_s).toString(),
                 u"https://esp32bench.dev/boards/esp32c3-supermini/"_s);
        // Typing is kept: the core is asked to save it (here there is no core, so
        // saving does nothing, and the text stays).
        view.myNotes()->setPlainText(u"LED on GPIO 4"_s);
        view.saveNotes();
        QCOMPARE(view.myNotes()->toPlainText(), u"LED on GPIO 4"_s);
    }

    void saysWhenThingsHappened()
    {
        const QDateTime now(QDate(2026, 11, 22), QTime(15, 0));
        QCOMPARE(Labels::ago(now.addSecs(-20), now), u"just now"_s);
        QCOMPARE(Labels::ago(now.addSecs(-5 * 60 - 3), now), u"5 min ago"_s);
        QCOMPARE(Labels::ago(now.addSecs(-3 * 3600), now), u"3 h ago"_s);
        QCOMPARE(Labels::ago(now.addDays(-2), now), QLocale().toString(now.addDays(-2), QLocale::ShortFormat));
        QCOMPARE(Labels::duration(12.34), u"%1 s"_s.arg(QLocale().toString(12.3, 'f', 1)));
        QCOMPARE(Labels::duration(125), u"2 min 05 s"_s);
    }

    void copiesALinkToTheProjectAndBoard()
    {
        CoreConnection core;
        ProjectView view(&core);
        view.showResult(example());
        QVERIFY(!view.copyLinkButton()->isHidden());
        view.copyLinkButton()->click();
        QCOMPARE(QGuiApplication::clipboard()->text(),
                 u"esp32bench://project/%1?board=%2"_s.arg(view.currentProjectId(), view.currentBuildBoard()));
        QCOMPARE(Labels::projectLink(u"blink"_s), u"esp32bench://project/blink"_s);
        QCOMPARE(Labels::projectLink(u"blink"_s, u"esp32c3 mini"_s), u"esp32bench://project/blink?board=esp32c3%20mini"_s);
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
        // A build of a minute ago, said as such.
        view.showBuildState(QJsonObject{{u"built"_s, true},
                                        {u"builtAt"_s, QDateTime::currentSecsSinceEpoch() - 90},
                                        {u"current"_s, true}});
        QVERIFY(view.buildStateText().contains(u"Built 1 min ago"_s));
    }

    void marksConnectedBoardsAndOpensInTheIde()
    {
        CoreConnection core;
        ProjectView view(&core);
        view.showResult(example());
        const QString first = view.boardPicker()->itemText(0);
        const QString chip = view.boardPicker()->itemData(0, Qt::UserRole + 1).toString();
        view.setConnectedChips({chip});
        QVERIFY(view.boardPicker()->itemText(0).endsWith(u" · connected"_s));
        QCOMPARE(view.currentBoardName(), first); // without the mark
        view.setConnectedChips({});
        QCOMPARE(view.boardPicker()->itemText(0), first);
        QVERIFY(!view.openIdeButton()->isHidden());
        QSignalSpy open(&view, &ProjectView::openInIdeRequested);
        view.openIdeButton()->click();
        QCOMPARE(open.size(), 1);
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
