// DevicesPanel renders device.list results (shape from schema/rpc/device.list.json).

#include "CoreConnection.h"
#include "DevicesPanel.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QDateTime>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

class TestDevicesPanel : public QObject {
    Q_OBJECT

private slots:
    void rendersPorts()
    {
        CoreConnection core;
        DevicesPanel panel(&core);
        const QJsonArray ports = QJsonDocument::fromJson(R"([
          {"path": "/dev/ttyACM0", "kind": "usb",
           "usb": {"vid": 12346, "pid": 4097, "product": "USB JTAG/serial debug unit"},
           "bridge": "usb-serial-jtag", "chips": ["esp32s3", "esp32c3", "esp32c6"], "access": "ok"},
          {"path": "/dev/ttyUSB0", "kind": "usb", "usb": {"vid": 4292, "pid": 60000},
           "bridge": "cp210x", "chips": [], "access": "denied"}
        ])").array();
        panel.showPorts(ports);

        QTreeWidget *t = panel.tree();
        QCOMPARE(t->topLevelItemCount(), 2);
        QTreeWidgetItem *jtag = t->topLevelItem(0);
        QCOMPARE(jtag->text(DevicesPanel::PortColumn), u"/dev/ttyACM0"_s);
        QCOMPARE(jtag->text(DevicesPanel::ChipsColumn), u"ESP32-S3, ESP32-C3, ESP32-C6"_s);
        QCOMPARE(jtag->text(DevicesPanel::UsbIdColumn), u"303a:1001"_s);
        QCOMPARE(jtag->toolTip(DevicesPanel::BridgeColumn), u"USB JTAG/serial debug unit, USB ID 303a:1001"_s);
        QVERIFY(t->isColumnHidden(DevicesPanel::UsbIdColumn)); // shown in the tooltip instead

        QTreeWidgetItem *cp = t->topLevelItem(1);
        QCOMPARE(cp->text(DevicesPanel::ChipsColumn), u"Any"_s);
        QCOMPARE(cp->text(DevicesPanel::UsbIdColumn), u"10c4:ea60"_s);
        QCOMPARE(cp->toolTip(DevicesPanel::BridgeColumn), u"USB ID 10c4:ea60"_s);
        QCOMPARE(cp->toolTip(DevicesPanel::PortColumn), DevicesPanel::accessHint());
#ifdef Q_OS_LINUX
        QVERIFY(cp->toolTip(DevicesPanel::PortColumn).contains(u"uucp"_s));
        QVERIFY(cp->toolTip(DevicesPanel::PortColumn).contains(u"dialout"_s));
#else
        // No udev or serial groups here: another program is the likely cause.
        QVERIFY(!cp->toolTip(DevicesPanel::PortColumn).contains(u"udev"_s));
#endif
        QVERIFY(panel.statusText().contains(u"cannot open: 1"_s));
    }

    void saysWhenNothingIsFound()
    {
        CoreConnection core;
        DevicesPanel panel(&core);
        panel.showPorts({});
        QCOMPARE(panel.tree()->topLevelItemCount(), 0);
        QVERIFY(panel.statusText().contains(u"No boards found"_s));
    }

    void namesAChipABoardCheckFound()
    {
        CoreConnection core;
        DevicesPanel panel(&core);
        panel.showPorts(QJsonArray{
            QJsonObject{{u"path"_s, u"/dev/ttyUSB0"_s}, {u"kind"_s, u"usb"_s}, {u"bridge"_s, u"cp210x"_s},
                        {u"chips"_s, QJsonArray{u"esp32"_s}}, {u"access"_s, u"ok"_s}, {u"identified"_s, true}},
            QJsonObject{{u"path"_s, u"/dev/ttyUSB1"_s}, {u"kind"_s, u"usb"_s}, {u"bridge"_s, u"ch340"_s},
                        {u"chips"_s, QJsonArray{}}, {u"access"_s, u"ok"_s}}});
        QTreeWidgetItem *known = panel.tree()->topLevelItem(0);
        QTreeWidgetItem *unknown = panel.tree()->topLevelItem(1);
        QCOMPARE(known->text(DevicesPanel::ChipsColumn), u"ESP32"_s);
        QVERIFY(known->toolTip(DevicesPanel::ChipsColumn).contains(u"board check"_s));
        QVERIFY(!known->data(DevicesPanel::ChipsColumn, DevicesPanel::UnknownChipRole).toBool());
        QVERIFY(unknown->data(DevicesPanel::ChipsColumn, DevicesPanel::UnknownChipRole).toBool());
        QVERIFY(unknown->toolTip(DevicesPanel::ChipsColumn).contains(u"Identify the chip"_s));

        // What the last Inspect found, beside the chip.
        panel.showPorts(QJsonArray{QJsonObject{
            {u"path"_s, u"/dev/ttyUSB0"_s}, {u"kind"_s, u"usb"_s}, {u"bridge"_s, u"cp210x"_s},
            {u"chips"_s, QJsonArray{u"esp32c3"_s}}, {u"access"_s, u"ok"_s}, {u"identified"_s, true},
            {u"inspected"_s, QJsonObject{{u"flashBytes"_s, 4194304}, {u"mac"_s, u"34:85:18:aa:bb:cc"_s},
                                         {u"revision"_s, u"v0.4"_s}, {u"at"_s, 1795000000}}}}});
        const QString text = panel.tree()->topLevelItem(0)->text(DevicesPanel::ChipsColumn);
        QVERIFY2(text.startsWith(u"ESP32-C3 · "_s) && text.contains(u"MAC 34:85:18:aa:bb:cc"_s)
                     && text.contains(u"revision v0.4"_s) && text.contains(u"4"_s),
                 qPrintable(text));
        QVERIFY(panel.tree()->topLevelItem(0)->toolTip(DevicesPanel::ChipsColumn).startsWith(u"Found by Inspect on"_s));
        // And what it runs, the build last flashed there; the port's own text stays its path.
        panel.showPorts(QJsonArray{QJsonObject{
            {u"path"_s, u"/dev/ttyUSB0"_s}, {u"kind"_s, u"usb"_s}, {u"bridge"_s, u"cp210x"_s},
            {u"chips"_s, QJsonArray{u"esp32"_s}}, {u"access"_s, u"ok"_s},
            {u"flashed"_s, QJsonObject{{u"project"_s, u"blink"_s}, {u"board"_s, u"esp32"_s},
                                       {u"at"_s, QDateTime::currentSecsSinceEpoch() - 180}}}}});
        QTreeWidgetItem *runs = panel.tree()->topLevelItem(0);
        QCOMPARE(runs->text(DevicesPanel::PortColumn), u"/dev/ttyUSB0"_s);
        QCOMPARE(runs->text(DevicesPanel::ChipsColumn), u"ESP32 · runs blink"_s);
        QVERIFY2(runs->toolTip(DevicesPanel::ChipsColumn).contains(u"blink for esp32 was flashed here 3 min ago."_s),
                 qPrintable(runs->toolTip(DevicesPanel::ChipsColumn)));
    }

    void explainsWhyNoBoardShows()
    {
        CoreConnection core;
        DevicesPanel panel(&core);
        QVERIFY(panel.adviceShown().isEmpty());
        panel.setAdvice(QJsonArray{QJsonObject{{u"kind"_s, u"data-cable"_s}},
                                   QJsonObject{{u"kind"_s, u"install-driver"_s}},
                                   QJsonObject{{u"kind"_s, u"bridge-without-port"_s}, {u"bridge"_s, u"ch340"_s}},
                                   QJsonObject{{u"kind"_s, u"something-newer"_s}}});
        const QString shown = panel.adviceShown();
        QVERIFY(shown.contains(u"charge-only"_s));
        QVERIFY(shown.contains(u"silabs.com"_s));
        QVERIFY(shown.contains(u"CH340"_s));
        QVERIFY(shown.contains(u"brltty"_s));
        // Advice this app does not know yet is left out.
        QCOMPARE(shown.count(u"<br>"_s), 2);
        panel.setAdvice({});
        QVERIFY(panel.adviceShown().isEmpty());
        QCOMPARE(DevicesPanel::adviceText(QJsonObject{{u"kind"_s, u"serial-access"_s}}), DevicesPanel::accessHint());
    }

    void showsDisconnectedState()
    {
        CoreConnection core;
        DevicesPanel panel(&core);
        QVERIFY(panel.statusText().contains(u"Not connected"_s));
    }
};

QTEST_MAIN(TestDevicesPanel)
#include "tst_devicespanel.moc"
