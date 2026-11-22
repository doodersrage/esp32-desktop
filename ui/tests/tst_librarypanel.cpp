// LibraryPanel renders library.list results (shape from schema/rpc/library.list.json).

#include "CoreConnection.h"
#include "LibraryPanel.h"

#include <QCheckBox>
#include <QFile>
#include <QHeaderView>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLineEdit>
#include <QComboBox>
#include <QSignalSpy>
#include <QSettings>
#include <QStandardPaths>
#include <QPushButton>
#include <QCoreApplication>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

class TestLibraryPanel : public QObject {
    Q_OBJECT

private:
    // A library.list result, from tests/data/library.json.
    static QJsonObject library()
    {
        QFile f(QStringLiteral(ESP32BENCH_SOURCE_DIR "/tests/data/library.json"));
        if (!f.open(QIODevice::ReadOnly))
            qFatal("cannot open %s", qPrintable(f.fileName()));
        return QJsonDocument::fromJson(f.readAll()).object();
    }

private slots:
    // Keep QSettings away from the real configuration; each test starts unfiltered.
    void initTestCase()
    {
        // An organisation, so QSettings can save: on Windows it is the registry key
        // Software\<organisation>\<app>, which an empty name does not make.
        QCoreApplication::setOrganizationName(u"esp32bench-tests"_s);
        QStandardPaths::setTestModeEnabled(true);
    }
    void init() { QSettings().remove(u"library"_s); }

    void countsRemembersAndClearsFilters()
    {
        CoreConnection core;
        {
            LibraryPanel panel(&core);
            panel.showLibrary(library());
            QVERIFY(panel.countText().isEmpty()); // all shown
            QVERIFY(panel.clearFiltersButton()->isHidden());
            panel.filter()->setText(u"heartbeat"_s);
            QCOMPARE(panel.countText(), u"Showing 1 of 2 projects."_s);
            QVERIFY(!panel.clearFiltersButton()->isHidden());
            panel.filter()->setText(u"nothing like this"_s);
            QCOMPARE(panel.countText(), u"No project matches these filters (2 in the library)."_s);
            panel.filter()->setText(u"heartbeat"_s);
            panel.simulatorFilter()->setChecked(true);
        }
        // The next start has them back...
        LibraryPanel panel(&core);
        QCOMPARE(panel.filter()->text(), u"heartbeat"_s);
        QVERIFY(panel.simulatorFilter()->isChecked());
        panel.showLibrary(library());
        QVERIFY(!panel.countText().isEmpty());
        // ...and Clear filters shows everything.
        panel.clearFiltersButton()->click();
        QVERIFY(panel.filter()->text().isEmpty());
        QVERIFY(!panel.simulatorFilter()->isChecked());
        QVERIFY(panel.countText().isEmpty());
        QVERIFY(!QSettings().value(u"library/simOnly"_s).toBool());
    }

    void rendersProjects()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        panel.showLibrary(library());
        QTreeWidget *t = panel.tree();
        QCOMPARE(t->topLevelItemCount(), 2);
        QTreeWidgetItem *blink = t->topLevelItem(0);
        QCOMPARE(blink->text(LibraryPanel::NameColumn), u"Blink and heartbeat"_s);
        QCOMPARE(blink->text(LibraryPanel::TimeColumn), u"5 min"_s);
        QCOMPARE(blink->text(LibraryPanel::FirmwareColumn), u"Arduino sketch"_s);
        QVERIFY(blink->toolTip(LibraryPanel::NameColumn).startsWith(u"Blink the onboard LED."_s));
        QCOMPARE(blink->data(LibraryPanel::NameColumn, LibraryPanel::UrlRole).toString(),
                 u"https://esp32bench.dev/projects/blink"_s);
        QCOMPARE(t->topLevelItem(1)->text(LibraryPanel::ChipsColumn), u"ESP32-S3, ESP32-C3"_s);
        QVERIFY(panel.statusText().contains(u"Projects: 2"_s));
    }

    void nameColumnFitsAndCanBeDragged()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        // Wide enough for the longest name with any system font.
        panel.resize(900, 300);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        panel.showLibrary(library());
        QTreeWidget *t = panel.tree();
        QHeaderView *h = t->header();
        // Every column can be dragged, the name included.
        QCOMPARE(h->sectionResizeMode(LibraryPanel::NameColumn), QHeaderView::Interactive);
        // At first the name column is as wide as the longest name, so none is cut off.
        const int longest = t->fontMetrics().horizontalAdvance(u"Blink and heartbeat"_s);
        QVERIFY2(t->columnWidth(LibraryPanel::NameColumn) > longest,
                 qPrintable(u"%1 <= %2"_s.arg(t->columnWidth(LibraryPanel::NameColumn)).arg(longest)));
        QVERIFY(panel.saveColumns().isEmpty()); // nothing the user chose yet

        // A width the user drags stays, through a new list and a resize, and is saved.
        h->resizeSection(LibraryPanel::NameColumn, 300);
        panel.showLibrary(library());
        panel.resize(1000, 300);
        QCOMPARE(t->columnWidth(LibraryPanel::NameColumn), 300);
        const QByteArray saved = panel.saveColumns();
        QVERIFY(!saved.isEmpty());

        LibraryPanel again(&core);
        again.restoreColumns(saved);
        QCOMPARE(again.tree()->columnWidth(LibraryPanel::NameColumn), 300);
        QVERIFY(again.tree()->isColumnHidden(LibraryPanel::TimeColumn));
        // Reset layout fits it again.
        panel.resetColumns();
        QVERIFY(panel.saveColumns().isEmpty());
        QVERIFY(t->columnWidth(LibraryPanel::NameColumn) > longest);
    }

    void filtersOnTheSimulatorAndTheConnectedBoard()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        panel.showLibrary(library());
        QTreeWidget *t = panel.tree();
        const auto shown = [t] {
            QStringList ids;
            for (int i = 0; i < t->topLevelItemCount(); ++i) {
                if (!t->topLevelItem(i)->isHidden())
                    ids << t->topLevelItem(i)->data(LibraryPanel::NameColumn, LibraryPanel::IdRole).toString();
            }
            return ids;
        };
        // Marked, and filtered on.
        QVERIFY(t->topLevelItem(0)->data(LibraryPanel::NameColumn, LibraryPanel::SimulatorRole).toBool());
        QVERIFY(t->topLevelItem(0)->toolTip(LibraryPanel::NameColumn).contains(u"Runs in the simulator"_s));
        panel.simulatorFilter()->setChecked(true);
        QCOMPARE(shown(), QStringList{u"blink"_s});
        panel.simulatorFilter()->setChecked(false);

        // No board that says its chip: the board filter is off.
        QVERIFY(!panel.boardFilter()->isEnabled());
        emit core.devicesListed(QJsonDocument::fromJson(R"([{"path": "/dev/ttyUSB0", "chips": []}])").array());
        QVERIFY(!panel.boardFilter()->isEnabled());
        // An S3 plugged in: only the project with an S3 build.
        emit core.devicesListed(QJsonDocument::fromJson(R"([{"path": "/dev/ttyACM0", "chips": ["esp32s3"]}])").array());
        QVERIFY(panel.boardFilter()->isEnabled());
        panel.boardFilter()->setChecked(true);
        QCOMPARE(shown(), QStringList{u"weather"_s});
        // Unplugged: the filter has nothing to go on, so everything shows again.
        emit core.devicesListed(QJsonArray{});
        QCOMPARE(shown(), (QStringList{u"blink"_s, u"weather"_s}));
    }

    void showsLocalProjectsFirst()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        const QJsonArray local{QJsonObject{{u"id"_s, u"local-3f9a0c21b4de"_s},
                                           {u"name"_s, u"blinky"_s},
                                           {u"path"_s, u"/home/u/esp/blinky"_s}}};
        panel.showLocal(local);
        panel.showLibrary(library());
        QTreeWidget *t = panel.tree();
        QCOMPARE(t->topLevelItemCount(), 3);
        QTreeWidgetItem *mine = t->topLevelItem(0);
        QCOMPARE(mine->text(LibraryPanel::NameColumn), u"blinky"_s);
        QCOMPARE(mine->text(LibraryPanel::FirmwareColumn), u"ESP-IDF (your folder)"_s);
        QVERIFY(mine->data(LibraryPanel::NameColumn, LibraryPanel::LocalRole).toBool());
        QCOMPARE(mine->toolTip(LibraryPanel::NameColumn), u"/home/u/esp/blinky"_s);
        // A filter finds it by its folder, and the library refreshing keeps it.
        panel.filter()->setText(u"esp/blinky"_s);
        QVERIFY(!mine->isHidden());
        panel.filter()->clear();
        panel.showLibrary(library());
        QCOMPARE(t->topLevelItemCount(), 3);
        QVERIFY(t->topLevelItem(0)->data(LibraryPanel::NameColumn, LibraryPanel::LocalRole).toBool());
        // Forgotten: gone, and the library's projects stay.
        panel.showLocal({});
        QCOMPARE(t->topLevelItemCount(), 2);
    }

    void starredProjectsComeFirstAndFilter()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        QJsonObject lib = library();
        QJsonArray projects = lib.value(u"projects"_s).toArray();
        QJsonObject weather = projects.at(1).toObject();
        weather.insert(u"starred"_s, true);
        projects.replace(1, weather);
        lib.insert(u"projects"_s, projects);
        panel.showLibrary(lib);
        QTreeWidget *t = panel.tree();
        QTreeWidgetItem *first = t->topLevelItem(0);
        QCOMPARE(first->data(LibraryPanel::NameColumn, LibraryPanel::IdRole).toString(), u"weather"_s);
        QVERIFY(first->data(LibraryPanel::NameColumn, LibraryPanel::StarredRole).toBool());
        QVERIFY(first->text(LibraryPanel::NameColumn).startsWith(u"★ "_s));
        QVERIFY(!t->topLevelItem(1)->data(LibraryPanel::NameColumn, LibraryPanel::StarredRole).toBool());

        panel.starredFilter()->setChecked(true);
        QVERIFY(!first->isHidden());
        QVERIFY(t->topLevelItem(1)->isHidden());
        panel.starredFilter()->setChecked(false);
        QVERIFY(!t->topLevelItem(1)->isHidden());
    }

    void filtersByTopicAndBuilt()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        QJsonObject lib = library();
        QJsonArray projects = lib.value(u"projects"_s).toArray();
        QJsonObject blink = projects.at(0).toObject();
        blink.insert(u"built"_s, QJsonArray{u"esp32"_s});
        blink.insert(u"libraries"_s, QJsonArray{u"Adafruit NeoPixel"_s});
        projects.replace(0, blink);
        lib.insert(u"projects"_s, projects);
        panel.showLibrary(lib);
        QTreeWidget *t = panel.tree();
        QTreeWidgetItem *first = t->topLevelItem(0);
        QVERIFY(first->font(LibraryPanel::NameColumn).bold());
        QVERIFY(first->toolTip(LibraryPanel::NameColumn).contains(u"Built on this computer for: esp32"_s));
        panel.builtFilter()->setChecked(true);
        QVERIFY(!first->isHidden());
        QVERIFY(t->topLevelItem(1)->isHidden());
        panel.builtFilter()->setChecked(false);
        // Topics: each tag, most used first, with how many projects have it.
        QVERIFY(panel.tagFilter()->count() > 1);
        const QString tag = panel.tagFilter()->itemData(1).toString();
        panel.tagFilter()->setCurrentIndex(1);
        for (int i = 0; i < t->topLevelItemCount(); ++i) {
            QTreeWidgetItem *item = t->topLevelItem(i);
            QCOMPARE(item->isHidden(), !item->data(LibraryPanel::NameColumn, LibraryPanel::TagsRole).toStringList().contains(tag));
        }
        // Parts and libraries are searched too.
        panel.tagFilter()->setCurrentIndex(0);
        panel.filter()->setText(u"neopixel"_s);
        QVERIFY(!first->isHidden());
        QVERIFY(t->topLevelItem(1)->isHidden());
        panel.filter()->clear();
        // Double-click or Enter asks for the project to run.
        QSignalSpy activated(&panel, &LibraryPanel::projectActivated);
        emit t->itemActivated(first, 0);
        QCOMPARE(activated.size(), 1);
    }

    void sortsTimeByMinutes()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        QJsonObject lib = library();
        QJsonArray ps = lib.value(u"projects"_s).toArray();
        QJsonObject third = ps.at(0).toObject();
        third[u"id"_s] = u"third"_s;
        third[u"name"_s] = u"Third"_s;
        third[u"minutes"_s] = 120; // "120 min" sorts before "45 min" as text
        ps.append(third);
        lib[u"projects"_s] = ps;
        panel.showLibrary(lib);
        panel.tree()->sortByColumn(LibraryPanel::TimeColumn, Qt::AscendingOrder);
        QCOMPARE(panel.tree()->topLevelItem(2)->text(LibraryPanel::TimeColumn), u"120 min"_s);
    }

    void filtersOnNameSummaryTagsAndChips()
    {
        CoreConnection core;
        LibraryPanel panel(&core);
        panel.showLibrary(library());
        auto visible = [&] {
            QStringList v;
            for (int i = 0; i < panel.tree()->topLevelItemCount(); ++i)
                if (!panel.tree()->topLevelItem(i)->isHidden())
                    v << panel.tree()->topLevelItem(i)->text(0);
            return v;
        };
        panel.filter()->setText(u"bme280"_s);
        QCOMPARE(visible(), QStringList{u"Weather station"_s});
        panel.filter()->setText(u"no-parts"_s);
        QCOMPARE(visible(), QStringList{u"Blink and heartbeat"_s});
        panel.filter()->setText(u"ESP32-C3 sensors"_s); // every word must match
        QCOMPARE(visible(), QStringList{u"Weather station"_s});
        panel.filter()->clear();
        QCOMPARE(visible().size(), 2);
    }

    void saysWhenNotDownloaded()
    {
        CoreConnection core; // not connected, so no automatic refresh
        LibraryPanel panel(&core);
        panel.showLibrary(QJsonObject{{u"available"_s, false}, {u"projects"_s, QJsonArray{}}});
        QCOMPARE(panel.tree()->topLevelItemCount(), 0);
        QVERIFY(panel.statusText().contains(u"not been downloaded"_s));
    }
};

QTEST_MAIN(TestLibraryPanel)
#include "tst_librarypanel.moc"
