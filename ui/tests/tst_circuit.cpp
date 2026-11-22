// CircuitPanel and CoreConnection's circuit, model and control calls against a fake
// core.

#include "CircuitPanel.h"
#include "CoreConnection.h"

#include <QDoubleSpinBox>
#include <QGraphicsItem>
#include <QGraphicsView>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QMenu>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QApplication>
#include <QMouseEvent>
#include <QDialog>
#include <QSpinBox>
#include <QTimer>
#include <QTest>
#include <QToolButton>

using namespace Qt::StringLiterals;

static QJsonObject model(const QString &id, const QString &name, const QString &look, const QJsonArray &pins,
                         const QJsonArray &controls, const QJsonObject &state)
{
    return {{u"schemaVersion"_s, u"1.0.0"_s}, {u"id"_s, id}, {u"name"_s, name}, {u"look"_s, look},
            {u"pins"_s, pins}, {u"controls"_s, controls}, {u"state"_s, state}, {u"script"_s, id + u".rhai"_s},
            {u"props"_s, QJsonObject{}}};
}

class FakeCore : public QObject {
    Q_OBJECT
public:
    QJsonObject saved{{u"schemaVersion"_s, u"1.1.0"_s},
                      {u"parts"_s, QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"kind"_s, u"led"_s}, {u"pin"_s, 2}}}}};
    QList<QJsonObject> controls;
    bool rejectNext = false;
    // What circuit.suggest answers.
    QJsonObject suggestion{{u"circuit"_s, QJsonObject{{u"schemaVersion"_s, u"1.2.0"_s}, {u"parts"_s, QJsonArray{}}}},
                           {u"unmatched"_s, QJsonArray{}}};

    explicit FakeCore(const QString &path)
        : m_server(new QLocalServer(this))
    {
        QLocalServer::removeServer(path);
        QVERIFY(m_server->listen(path));
        connect(m_server, &QLocalServer::newConnection, this, [this] {
            m_sock = m_server->nextPendingConnection();
            connect(m_sock, &QLocalSocket::readyRead, m_sock, [this] {
                while (m_sock->canReadLine())
                    handle(QJsonDocument::fromJson(m_sock->readLine()).object());
            });
        });
    }

    void picture(const QString &id, const QByteArray &pixels, int w, int h)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.partPicture"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"id"_s, id}, {u"width"_s, w}, {u"height"_s, h},
                                        {u"data"_s, QString::fromLatin1(pixels.toBase64())}}}});
    }

    void parts(const QJsonArray &changed)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.parts"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"parts"_s, changed}}}});
    }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        const QJsonObject p = req.value(u"params"_s).toObject();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.33.0"_s}});
        } else if (m == u"circuit.models") {
            const QJsonObject off{{u"on"_s, false}};
            reply(id, QJsonObject{{u"models"_s, QJsonArray{
                model(u"led"_s, u"LED"_s, u"led"_s, {u"anode"_s}, {}, off),
                model(u"button"_s, u"Push button"_s, u"button"_s, {u"pin"_s},
                      {QJsonObject{{u"name"_s, u"press"_s}, {u"type"_s, u"momentary"_s}, {u"label"_s, u"Press"_s}}}, off),
                model(u"switch"_s, u"Slide switch"_s, u"switch"_s, {u"pin"_s},
                      {QJsonObject{{u"name"_s, u"toggle"_s}, {u"type"_s, u"toggle"_s}, {u"label"_s, u"On"_s}}}, off),
                model(u"rgb-led"_s, u"RGB LED"_s, u"rgb-led"_s, {u"red"_s, u"green"_s, u"blue"_s}, {},
                      QJsonObject{{u"red"_s, false}, {u"green"_s, false}, {u"blue"_s, false}}),
                [&] {
                    QJsonObject m = model(u"bme280"_s, u"BME280"_s, u"box"_s, {},
                                          {QJsonObject{{u"name"_s, u"temperature"_s}, {u"type"_s, u"slider"_s},
                                                       {u"label"_s, u"Temperature"_s}, {u"min"_s, -40}, {u"max"_s, 85},
                                                       {u"default"_s, 21}, {u"unit"_s, u"°C"_s}}},
                                          QJsonObject{{u"temperature"_s, 21.0}});
                    m.insert(u"i2c"_s, QJsonObject{{u"addresses"_s, QJsonArray{118, 119}}});
                    return m;
                }(),
                [&] {
                    QJsonObject m = model(u"ssd1306"_s, u"SSD1306"_s, u"display"_s, {}, {}, off);
                    m.insert(u"i2c"_s, QJsonObject{{u"addresses"_s, QJsonArray{60, 61}}});
                    m.insert(u"display"_s, QJsonObject{{u"width"_s, 4}, {u"height"_s, 2}});
                    return m;
                }(),
                [&] {
                    QJsonObject m = model(u"ws2812"_s, u"WS2812 LED strip"_s, u"strip"_s, {u"din"_s}, {},
                                          QJsonObject{{u"leds"_s, QJsonArray{}}});
                    m.insert(u"props"_s, QJsonObject{{u"count"_s, QJsonObject{{u"type"_s, u"int"_s}, {u"default"_s, 8},
                                                                                {u"label"_s, u"LEDs"_s}}}});
                    return m;
                }(),
                [&] {
                    QJsonObject m = model(u"ds3231"_s, u"DS3231"_s, u"box"_s, {u"sqw"_s}, {}, QJsonObject{});
                    m.insert(u"optionalPins"_s, QJsonArray{u"sqw"_s});
                    m.insert(u"i2c"_s, QJsonObject{{u"addresses"_s, QJsonArray{104}}});
                    return m;
                }(),
            }}});
        } else if (m == u"circuit.suggest") {
            reply(id, suggestion);
        } else if (m == u"circuit.get") {
            reply(id, QJsonObject{{u"circuit"_s, saved}});
        } else if (m == u"circuit.set") {
            if (rejectNext) {
                rejectNext = false;
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32602}, {u"message"_s, u"two parts would drive GPIO 4"_s}}}});
                return;
            }
            saved = p.value(u"circuit"_s).toObject();
            reply(id, QJsonObject{});
        } else if (m == u"sim.start") {
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, false},
                                  {u"engine"_s, u"lcgamboa"_s}, {u"pins"_s, QJsonArray{2, 4}}, {u"partsActive"_s, true}});
        } else if (m == u"sim.control") {
            controls << p;
            reply(id, QJsonObject{});
        } else {
            reply(id, QJsonObject{});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestCircuit : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

    static QJsonObject partsOf(const QJsonObject &circuit, int i)
    {
        return circuit.value(u"parts"_s).toArray().at(i).toObject();
    }

private slots:
    void drawsALedStrip()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QTRY_VERIFY(panel.modelIds().contains(u"ws2812"_s));
        panel.showBoard(u"neopixel"_s, u"esp32"_s, QJsonArray{u"16"_s, u"GND"_s});
        QTRY_COMPARE(panel.parts().size(), 1);

        // Stopped, the strip is drawn as long as its count says.
        panel.addPart(u"ws2812"_s, QJsonObject{{u"din"_s, 16}}, QJsonObject{{u"count"_s, 3}}, {});
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_COMPARE(panel.partStripLength(u"ws2812-1"_s), 3);
        QCOMPARE(panel.partStripLength(u"led-1"_s), 0);

        // Running: each LED shows the colour the core reports.
        core.startSim(u"neopixel"_s, u"esp32"_s);
        QTRY_VERIFY(core.simPartsActive());
        const QJsonArray leds{QJsonArray{40, 0, 0}, QJsonArray{0, 0, 40}, QJsonArray{0, 0, 0}};
        fake.parts(QJsonArray{QJsonObject{{u"id"_s, u"ws2812-1"_s}, {u"state"_s, QJsonObject{{u"leds"_s, leds}}}}});
        QTRY_COMPARE(panel.partState(u"ws2812-1"_s).value(u"leds"_s).toArray(), leds);
        QCOMPARE(panel.partStripLength(u"ws2812-1"_s), 3);
        core.stopSim();
        QTRY_VERIFY(!core.isSimRunning());
    }

    void offersTheProjectsPartsOnAnEmptyBoard()
    {
        FakeCore fake(path());
        fake.saved = QJsonObject{{u"schemaVersion"_s, u"1.2.0"_s}, {u"parts"_s, QJsonArray{}}};
        const QJsonObject wired{
            {u"schemaVersion"_s, u"1.2.0"_s},
            {u"parts"_s, QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"kind"_s, u"led"_s}, {u"pin"_s, 16},
                                                {u"label"_s, u"Green LED"_s}},
                                    QJsonObject{{u"id"_s, u"button-1"_s}, {u"kind"_s, u"button"_s}, {u"pin"_s, 25},
                                                {u"label"_s, u"Green button"_s},
                                                {u"props"_s, QJsonObject{{u"activeLow"_s, true}}}}}}};
        fake.suggestion = {{u"circuit"_s, wired}, {u"unmatched"_s, QJsonArray{u"Buzzer"_s}}};
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.show();
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        panel.showBoard(u"simon"_s, u"esp32"_s, QJsonArray{u"16"_s, u"25"_s});
        // Empty board: the wiring's parts are offered, and what is not simulated is said.
        QTRY_VERIFY(panel.addSuggestedButton()->isVisibleTo(&panel));
        QVERIFY(panel.statusText().contains(u"Green LED, Green button"_s));
        QVERIFY(panel.statusText().contains(u"Not simulated: Buzzer."_s));
        QVERIFY(panel.parts().isEmpty()); // nothing placed by asking
        panel.addSuggestedButton()->click();
        QTRY_COMPARE(panel.parts().size(), 2);
        QCOMPARE(fake.saved, wired);
        QVERIFY(!panel.addSuggestedButton()->isVisibleTo(&panel));

        // A board that has parts is left alone.
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s});
        QTRY_COMPARE(panel.parts().size(), 2);
        QTest::qWait(100);
        QVERIFY(!panel.addSuggestedButton()->isVisibleTo(&panel));
    }

    void dragsPartsAndUndoes()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(700, 500);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QTRY_VERIFY(!panel.modelIds().isEmpty());
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QVERIFY(!panel.undoButton()->isEnabled()); // nothing changed yet

        // Drag the LED with the mouse: its place is saved (circuit 1.3's at).
        QGraphicsView *view = panel.view();
        const QPoint from = view->mapFromScene(panel.partCentre(u"led-1"_s));
        const QPoint to = from + QPoint(60, 40);
        QTest::mousePress(view->viewport(), Qt::LeftButton, {}, from);
        QMouseEvent move(QEvent::MouseMove, to, view->viewport()->mapToGlobal(to), Qt::NoButton, Qt::LeftButton, {});
        QApplication::sendEvent(view->viewport(), &move);
        QTest::mouseRelease(view->viewport(), Qt::LeftButton, {}, to);
        QTRY_VERIFY(partsOf(fake.saved, 0).contains(u"at"_s));
        const QJsonObject at = partsOf(fake.saved, 0).value(u"at"_s).toObject();
        QVERIFY(at.value(u"x"_s).toDouble() > 0);
        QTRY_VERIFY(panel.undoButton()->isEnabled());

        // Undo puts it back where it was laid out.
        panel.undoButton()->click();
        QTRY_VERIFY(!partsOf(fake.saved, 0).contains(u"at"_s));
        QVERIFY(!panel.undoButton()->isEnabled());

        // From the keyboard: Tab reaches the LED, the arrows move it, and the place is
        // saved when the key is let go.
        panel.activateWindow();
        QVERIFY(QTest::qWaitForWindowActive(&panel));
        view->setFocus();
        QTest::keyClick(view, Qt::Key_Tab);
        QVERIFY(view->scene()->focusItem());
        QTest::keyClick(view, Qt::Key_Right);
        QTRY_VERIFY(partsOf(fake.saved, 0).contains(u"at"_s));
        panel.undoButton()->click();
        QTRY_VERIFY(!partsOf(fake.saved, 0).contains(u"at"_s));

        // An added part can be undone too.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        panel.undoButton()->click();
        QTRY_COMPARE(panel.parts().size(), 1);
    }

    void redoesDeletesAndDuplicates()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(700, 500);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QTRY_VERIFY(!panel.modelIds().isEmpty());
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s, u"5"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QVERIFY(!panel.redoButton()->isEnabled());

        // Redo makes again what Undo took back; a new change ends that.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        panel.undoButton()->click();
        QTRY_COMPARE(panel.parts().size(), 1);
        QVERIFY(panel.redoButton()->isEnabled());
        panel.redoButton()->click();
        QTRY_COMPARE(panel.parts().size(), 2);
        QVERIFY(!panel.redoButton()->isEnabled());

        // Delete on the canvas removes the selected part.
        panel.activateWindow();
        QVERIFY(QTest::qWaitForWindowActive(&panel));
        const QString button = panel.parts().at(1).toObject().value(u"id"_s).toString();
        panel.selectPart(button, true);
        QCOMPARE(panel.selectedPartId(), button);
        QVERIFY(panel.duplicateButton()->isEnabled());
        panel.view()->setFocus();
        QTest::keyClick(panel.view(), Qt::Key_Delete);
        QTRY_COMPARE(panel.parts().size(), 1);

        // Duplicate: the add dialog, filled from the LED, on the project pin it leaves free.
        QTRY_VERIFY(panel.selectedPartId().isEmpty() || panel.selectedPartId() == u"led-1"_s);
        panel.selectPart(u"led-1"_s, true);
        QTRY_COMPARE(panel.selectedPartId(), u"led-1"_s);
        int offered = -1;
        QTimer::singleShot(0, this, [&offered] {
            auto *dialog = qobject_cast<QDialog *>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            const auto boxes = dialog->findChildren<QSpinBox *>();
            QVERIFY(!boxes.isEmpty());
            offered = boxes.first()->value();
            dialog->accept();
        });
        QTest::keyClick(panel.view(), Qt::Key_D, Qt::ControlModifier);
        QCOMPARE(offered, 5);
        QTRY_COMPARE(panel.parts().size(), 2);
        const QJsonObject copy = panel.parts().at(1).toObject();
        QCOMPARE(copy.value(u"kind"_s).toString(), u"led"_s);
        QCOMPARE(copy.value(u"pins"_s).toObject().begin().value().toInt(), 5);
    }

    void zoomsAndFits()
    {
        CoreConnection core;
        CircuitPanel panel(&core);
        panel.resize(500, 400);
        QCOMPARE(panel.zoom(), 1.0);
        panel.zoomBy(1.25);
        QCOMPARE(panel.zoom(), 1.25);
        for (int i = 0; i < 20; ++i)
            panel.zoomBy(1.25);
        QCOMPARE(panel.zoom(), 4.0); // at most four times
        panel.fitView();
        QVERIFY(panel.zoom() <= 1.0); // never bigger than life
    }

    void editsAndRunsParts()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(700, 500);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        // The kinds of part come from the core, one menu entry each.
        QTRY_COMPARE(panel.modelIds(), (QStringList{u"led"_s, u"button"_s, u"switch"_s, u"rgb-led"_s, u"bme280"_s, u"ssd1306"_s, u"ws2812"_s, u"ds3231"_s}));
        QCOMPARE(panel.addPartButton()->menu()->actions().size(), 8);

        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s, u"GND"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QVERIFY(panel.item(u"led-1"_s));
        QVERIFY(panel.addPartButton()->isEnabled());

        // Adding saves through the core, with fresh ids.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        QJsonObject b = partsOf(fake.saved, 1);
        QCOMPARE(b.value(u"id"_s).toString(), u"button-1"_s);
        QCOMPARE(b.value(u"pins"_s).toObject().value(u"pin"_s).toInt(), 4);
        QCOMPARE(b.value(u"props"_s).toObject().value(u"activeLow"_s).toBool(), true);
        QCOMPARE(b.value(u"label"_s).toString(), u"Go"_s);
        QCOMPARE(fake.saved.value(u"schemaVersion"_s).toString(), u"1.3.0"_s);
        panel.addPart(u"rgb-led"_s, QJsonObject{{u"red"_s, 25}, {u"green"_s, 26}, {u"blue"_s, 27}}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 3);
        QCOMPARE(partsOf(fake.saved, 2).value(u"id"_s).toString(), u"rgb-led-1"_s);
        panel.addPart(u"switch"_s, QJsonObject{{u"pin"_s, 5}}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 4);
        // An I2C part: no pins, a bus and an address.
        panel.addPart(u"ssd1306"_s, {}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 60}});
        QTRY_COMPARE(panel.parts().size(), 5);
        QCOMPARE(partsOf(fake.saved, 4).value(u"i2c"_s).toObject().value(u"address"_s).toInt(), 60);
        QVERIFY(!partsOf(fake.saved, 4).contains(u"pins"_s));
        panel.addPart(u"bme280"_s, {}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 118}});
        QTRY_COMPARE(panel.parts().size(), 6);
        // An I2C part with a pin as well: it stays with the I2C parts, and names the pin.
        panel.addPart(u"ds3231"_s, QJsonObject{{u"sqw"_s, 25}}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 104}});
        QTRY_COMPARE(panel.parts().size(), 7);
        QTRY_COMPARE(panel.partToolTip(u"ds3231-1"_s), u"I2C bus 0, address 0x68, SQW on GPIO 25"_s);
        panel.removePart(u"ds3231-1"_s);
        QTRY_COMPARE(panel.parts().size(), 6);

        // A refusal from the core is shown, and nothing changes.
        fake.rejectNext = true;
        panel.addPart(u"switch"_s, 4, false, {});
        QTRY_VERIFY(panel.statusText().contains(u"would drive"_s));
        QCOMPARE(panel.parts().size(), 6);

        // Stopped, a sensor's controls set where the next run starts: saved in the
        // circuit, shown on the part, and the part stays selected.
        panel.selectPart(u"bme280-1"_s, true);
        QDoubleSpinBox *start = panel.controlBox(u"temperature"_s);
        QVERIFY(start);
        QVERIFY(start->isEnabled());
        QCOMPARE(start->value(), 21.0);
        start->setValue(25.5);
        QTRY_COMPARE(partsOf(fake.saved, 5).value(u"controls"_s).toObject().value(u"temperature"_s).toDouble(), 25.5);
        QTRY_COMPARE(panel.partState(u"bme280-1"_s).value(u"temperature"_s).toDouble(), 25.5);
        QTRY_VERIFY(panel.controlBox(u"temperature"_s));
        QCOMPARE(panel.controlBox(u"temperature"_s)->value(), 25.5);
        QVERIFY(fake.controls.isEmpty());
        panel.selectPart(u"bme280-1"_s, false);

        // Running: parts show the state the core reports.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.simPartsActive());
        QTRY_VERIFY(panel.statusText().contains(u"Running"_s));
        fake.parts(QJsonArray{
            QJsonObject{{u"id"_s, u"led-1"_s}, {u"on"_s, true}, {u"state"_s, QJsonObject{{u"on"_s, true}}}},
            QJsonObject{{u"id"_s, u"rgb-led-1"_s},
                        {u"state"_s, QJsonObject{{u"red"_s, true}, {u"green"_s, false}, {u"blue"_s, true}}}}});
        QTRY_VERIFY(panel.isOn(u"led-1"_s));
        QCOMPARE(panel.partState(u"rgb-led-1"_s).value(u"blue"_s).toBool(), true);
        // A display part shows its picture: 4 x 2, one pixel lit at (3, 1).
        fake.picture(u"ssd1306-1"_s, QByteArray("\0\0\0\0\0\0\0\1", 8), 4, 2);
        QTRY_VERIFY(!panel.partPicture(u"ssd1306-1"_s).isNull());
        QCOMPARE(panel.partPicture(u"ssd1306-1"_s).size(), QSize(4, 2));
        QVERIFY(qGray(panel.partPicture(u"ssd1306-1"_s).pixel(3, 1)) > 128);
        QCOMPARE(qGray(panel.partPicture(u"ssd1306-1"_s).pixel(0, 0)), 0);

        // A sensor: selecting it shows its readings as controls; changing one sends it.
        QVERIFY(!panel.controlBox(u"temperature"_s));
        panel.selectPart(u"bme280-1"_s, true);
        QDoubleSpinBox *temp = panel.controlBox(u"temperature"_s);
        QVERIFY(temp);
        QVERIFY(temp->isEnabled());
        QCOMPARE(temp->value(), 25.5); // where it was left
        QCOMPARE(temp->maximum(), 85.0);
        temp->setValue(30.5);
        QTRY_VERIFY(!fake.controls.isEmpty());
        QCOMPARE(fake.controls.last().value(u"part"_s).toString(), u"bme280-1"_s);
        QCOMPARE(fake.controls.last().value(u"control"_s).toString(), u"temperature"_s);
        QCOMPARE(fake.controls.last().value(u"value"_s).toDouble(), 30.5);
        fake.parts(QJsonArray{QJsonObject{{u"id"_s, u"bme280-1"_s}, {u"state"_s, QJsonObject{{u"temperature"_s, 30.5}}}}});
        QTRY_COMPARE(panel.partState(u"bme280-1"_s).value(u"temperature"_s).toDouble(), 30.5);
        panel.selectPart(u"bme280-1"_s, false);
        QVERIFY(!panel.controlBox(u"temperature"_s));
        fake.controls.clear();

        // Holding the mouse on a button presses it.
        const QPoint at = panel.view()->mapFromScene(panel.partCentre(u"button-1"_s));
        QTest::mousePress(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTRY_COMPARE(fake.controls.size(), 1);
        QCOMPARE(fake.controls[0].value(u"part"_s).toString(), u"button-1"_s);
        QCOMPARE(fake.controls[0].value(u"control"_s).toString(), u"press"_s);
        QCOMPARE(fake.controls[0].value(u"value"_s).toBool(), true);
        QTest::mouseRelease(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTRY_COMPARE(fake.controls.size(), 2);
        QCOMPARE(fake.controls[1].value(u"value"_s).toBool(), false);
        // Clicking a switch flips it: on, since it is off.
        const QPoint sw = panel.view()->mapFromScene(panel.partCentre(u"switch-1"_s));
        QTest::mouseClick(panel.view()->viewport(), Qt::LeftButton, {}, sw);
        QTRY_COMPARE(fake.controls.size(), 3);
        QCOMPARE(fake.controls[2].value(u"control"_s).toString(), u"toggle"_s);
        QCOMPARE(fake.controls[2].value(u"value"_s).toBool(), true);
        // A part's error is shown.
        fake.parts(QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"error"_s, u"the script failed"_s}}});
        QTRY_VERIFY(panel.statusText().contains(u"the script failed"_s));

        core.stopSim();
        QTRY_VERIFY(!panel.isOn(u"led-1"_s));
        QCOMPARE(panel.partState(u"rgb-led-1"_s).value(u"blue"_s).toBool(), false);
        // Not running: nothing is sent.
        QTest::mousePress(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTest::mouseRelease(panel.view()->viewport(), Qt::LeftButton, {}, at);
        QTest::qWait(100);
        QCOMPARE(fake.controls.size(), 3);

        QVERIFY(panel.partPicture(u"ssd1306-1"_s).isNull()); // cleared when the simulator stopped
        panel.removePart(u"rgb-led-1"_s);
        QTRY_COMPARE(panel.parts().size(), 5);
    }
};

QTEST_MAIN(TestCircuit)
#include "tst_circuit.moc"
