// DevicesPanel renders device.list results (shape from schema/rpc/device.list.json).

#include "CoreConnection.h"
#include "DevicesPanel.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
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
