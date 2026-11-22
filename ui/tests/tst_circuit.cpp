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
#include <QClipboard>
#include <QGuiApplication>
#include <QMimeData>
#include <QImage>
#include <QSignalSpy>
#include <QGraphicsLineItem>
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
    QStringList saves; // each circuit.set, in order: where its first part is
    QJsonObject saved{{u"schemaVersion"_s, u"1.1.0"_s},
                      {u"parts"_s, QJsonArray{QJsonObject{{u"id"_s, u"led-1"_s}, {u"kind"_s, u"led"_s}, {u"pin"_s, 2}}}}};
    QList<QJsonObject> controls;
    bool rejectNext = false;
    // circuit.pinNotes' layout: the board's header (empty: none, as with no site data).
    QJsonObject layout;
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
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.47.0"_s}});
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
                    m.insert(u"wiringNotes"_s, QJsonArray{u"DIN: 300 to 500 Ω in series."_s});
                    // Drawn as the strip it is: its pads at one end, DIN wired.
                    m.insert(u"drawing"_s, QJsonObject{{u"board"_s, u"#1e1e1e"_s},
                                                       {u"header"_s, QJsonArray{
                                                           QJsonObject{{u"label"_s, u"5V"_s}, {u"connect"_s, u"power"_s}},
                                                           QJsonObject{{u"label"_s, u"DIN"_s}, {u"connect"_s, u"pin:din"_s}},
                                                           QJsonObject{{u"label"_s, u"GND"_s}, {u"connect"_s, u"ground"_s}}}}});
                    return m;
                }(),
                [&] {
                    QJsonObject m = model(u"ds3231"_s, u"DS3231"_s, u"box"_s, {u"sqw"_s}, {}, QJsonObject{});
                    m.insert(u"optionalPins"_s, QJsonArray{u"sqw"_s});
                    m.insert(u"i2c"_s, QJsonObject{{u"addresses"_s, QJsonArray{104}}});
                    return m;
                }(),
            }}});
        } else if (m == u"circuit.pinNotes") {
            QJsonObject notes{{u"chip"_s, u"esp32"_s},
                              {u"notes"_s, QJsonArray{QJsonObject{{u"gpio"_s, 12}, {u"kind"_s, u"strapping"_s}},
                                                      QJsonObject{{u"gpio"_s, 34}, {u"kind"_s, u"inputOnly"_s}}}}};
            if (!layout.isEmpty()) {
                notes.insert(u"source"_s, u"board"_s);
                notes.insert(u"layout"_s, layout);
            }
            reply(id, notes);
        } else if (m == u"circuit.export") {
            reply(id, QJsonObject{{u"path"_s, p.value(u"path"_s)}, {u"parts"_s, 1}});
        } else if (m == u"circuit.import") {
            if (p.value(u"path"_s).toString().endsWith(u"bad.json"_s)) {
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32602}, {u"message"_s, u"not a circuit file"_s}}}});
                return;
            }
            saved = QJsonObject{{u"schemaVersion"_s, u"1.3.0"_s},
                                {u"parts"_s, QJsonArray{QJsonObject{{u"id"_s, u"button-1"_s}, {u"kind"_s, u"button"_s}, {u"pin"_s, 4}}}}};
            reply(id, QJsonObject{{u"circuit"_s, saved}});
        } else if (m == u"circuit.examples") {
            reply(id, QJsonObject{{u"projects"_s, p.value(u"kind"_s).toString() == u"led"_s
                       ? QJsonArray{QJsonObject{{u"id"_s, u"blink"_s}, {u"name"_s, u"Blink"_s}, {u"boards"_s, QJsonArray{u"esp32"_s, u"esp32c3"_s}}},
                                    QJsonObject{{u"id"_s, u"traffic"_s}, {u"name"_s, u"Traffic light"_s}, {u"boards"_s, QJsonArray{u"esp32s3"_s}}}}
                       : QJsonArray{}}});
        } else if (m == u"circuit.suggest") {
            reply(id, suggestion);
        } else if (m == u"circuit.get") {
            reply(id, QJsonObject{{u"circuit"_s, saved}});
        } else if (m == u"circuit.set") {
            // A slow core (Windows CI's first runs): answer later, as it would.
            if (const int ms = qEnvironmentVariableIntValue("ESP32BENCH_TEST_SLOW_SAVE"); ms > 0 && !m_delaying) {
                m_delaying = true;
                QTimer::singleShot(ms, this, [this, req] {
                    handle(req);
                    m_delaying = false;
                });
                return;
            }
            if (rejectNext) {
                rejectNext = false;
                send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id},
                      {u"error"_s, QJsonObject{{u"code"_s, -32602}, {u"message"_s, u"two parts would drive GPIO 4"_s}}}});
                return;
            }
            saved = p.value(u"circuit"_s).toObject();
            // Each save, for a failure to say what came in: its first part's place.
            const QJsonObject first = saved.value(u"parts"_s).toArray().at(0).toObject();
            saves << (first.contains(u"at"_s) ? u"at %1,%2"_s.arg(first.value(u"at"_s).toObject().value(u"x"_s).toInt())
                                                     .arg(first.value(u"at"_s).toObject().value(u"y"_s).toInt())
                                              : u"laid out"_s);
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
    bool m_delaying = false;
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered

        // Stopped, the strip is drawn as long as its count says.
        panel.addPart(u"ws2812"_s, QJsonObject{{u"din"_s, 16}}, QJsonObject{{u"count"_s, 3}}, {});
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QCOMPARE(fake.saved, wired);
        QVERIFY(!panel.addSuggestedButton()->isVisibleTo(&panel));

        // A board that has parts is left alone.
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s});
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QTest::qWait(100);
        QVERIFY(!panel.addSuggestedButton()->isVisibleTo(&panel));
    }

    // A module is drawn with its header, and its wire lands on the right pin of it,
    // level with the board pin, with the rows making room so parts do not overlap.
    void drawsModulesWithTheirHeaders()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(900, 600);
        panel.show();
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QTRY_VERIFY(!panel.modelIds().isEmpty());
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s, u"5"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QTRY_VERIFY(!panel.isSaving());
        panel.addPart(u"ws2812"_s, QJsonObject{{u"din"_s, 5}}, {}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving());
        QTRY_VERIFY(panel.item(u"ws2812-1"_s));
        QVERIFY(panel.drawnAsModule(u"ws2812-1"_s));
        QVERIFY(!panel.drawnAsModule(u"led-1"_s)); // no drawing in its model
        QVERIFY(panel.partToolTip(u"ws2812-1"_s).contains(u"5V DIN GND"_s));
        // The site's note on wiring it (the app is in English here).
        QVERIFY(panel.partToolTip(u"ws2812-1"_s).contains(u"DIN: 300 to 500 Ω in series."_s));
        // The wire to GPIO 5 ends on DIN, and runs level.
        const QPointF din = panel.pinPoint(u"ws2812-1"_s, u"din"_s);
        bool wired = false;
        for (QGraphicsItem *i : panel.view()->scene()->items()) {
            if (auto *line = dynamic_cast<QGraphicsLineItem *>(i)) {
                if (line->line().p2() == din) {
                    wired = true;
                    QCOMPARE(line->line().p1().y(), din.y());
                }
            }
        }
        QVERIFY(wired);
        // The LED on GPIO 2's row stays clear of the strip on GPIO 5's.
        const QRectF led = panel.partRect(u"led-1"_s);
        const QRectF strip = panel.partRect(u"ws2812-1"_s);
        QVERIFY2(!led.intersects(strip), "parts on neighbouring rows overlap");
    }

    // With the board's header from the site, the board is drawn as made: parts beside
    // the pads they are wired to, on that side, I2C modules wired to the I2C pads.
    void drawsTheBoardFromItsHeader()
    {
        FakeCore fake(path());
        fake.layout = QJsonObject{{u"left"_s, QJsonArray{u"3V3"_s, u"2"_s, u"5"_s, u"GND"_s}},
                                  {u"right"_s, QJsonArray{u"GND"_s, u"21"_s, u"22"_s, u"4"_s}},
                                  {u"pads"_s, QJsonArray{}}, {u"gpioOf"_s, QJsonObject{}}, {u"silk"_s, QJsonObject{}},
                                  {u"i2c"_s, QJsonObject{{u"sda"_s, 21}, {u"scl"_s, 22}}},
                                  {u"safe"_s, QJsonArray{4, 5}}};
        CoreConnection core;
        core.setSocketPath(path());
        CircuitPanel panel(&core);
        panel.resize(900, 600);
        panel.show();
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QTRY_VERIFY(!panel.modelIds().isEmpty());
        panel.showBoard(u"blink"_s, u"esp32"_s, QJsonArray{u"2"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QTRY_VERIFY(panel.headerDrawn());
        panel.addPart(u"bme280"_s, {}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 0x76}});
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving());
        QTRY_VERIFY(panel.item(u"bme280-1"_s));
        // The LED, on a left-side pad, sits left of the board; the BME280 right of it.
        QVERIFY(panel.partRect(u"led-1"_s).right() < 0);
        QVERIFY(panel.partRect(u"bme280-1"_s).left() > 0);
        // Two wires end on the BME280: SDA and SCL, from the board's I2C pads.
        int ends = 0;
        for (QGraphicsItem *i : panel.view()->scene()->items()) {
            if (auto *line = dynamic_cast<QGraphicsLineItem *>(i))
                ends += panel.partRect(u"bme280-1"_s).contains(line->line().p2()) ? 1 : 0;
        }
        QCOMPARE(ends, 2);
        QVERIFY(!panel.partRect(u"led-1"_s).intersects(panel.partRect(u"bme280-1"_s)));
        // Another LED: the project's pin is taken, so it is offered the board's first safe pin.
        panel.selectPart(u"led-1"_s, true);
        QTRY_COMPARE(panel.selectedPartId(), u"led-1"_s);
        int offered = -1;
        QTimer::singleShot(0, this, [&offered] {
            auto *dialog = qobject_cast<QDialog *>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            const auto boxes = dialog->findChildren<QSpinBox *>();
            QVERIFY(!boxes.isEmpty());
            offered = boxes.first()->value();
            dialog->reject();
        });
        QTest::keyClick(panel.view(), Qt::Key_D, Qt::ControlModifier);
        QCOMPARE(offered, 4);
        // A pin off the header (GPIO 33) falls back to the plain drawing.
        panel.addPart(u"led"_s, 33, false, {});
        QTRY_COMPARE(panel.parts().size(), 3);
        QTRY_VERIFY(!panel.isSaving());
        QTRY_VERIFY(!panel.headerDrawn());
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        const QJsonObject at = partsOf(fake.saved, 0).value(u"at"_s).toObject();
        QVERIFY(at.value(u"x"_s).toDouble() > 0);
        QTRY_VERIFY(panel.undoButton()->isEnabled());

        // Undo puts it back where it was laid out.
        // Off while a save is unanswered: wait for it, as a person does.
        QTRY_VERIFY(panel.undoButton()->isEnabled());
        panel.undoButton()->click();
        QTRY_VERIFY2(!partsOf(fake.saved, 0).contains(u"at"_s), qPrintable(fake.saves.join(u" | "_s)));
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        // Off while a save is unanswered: wait for it, as a person does.
        QTRY_VERIFY(panel.undoButton()->isEnabled());
        panel.undoButton()->click();
        QTRY_VERIFY2(!partsOf(fake.saved, 0).contains(u"at"_s), qPrintable(fake.saves.join(u" | "_s)));
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        // Tidy: nothing to do when no part was moved; after a move, all back in place.
        QVERIFY(!panel.tidy());
        view->setFocus();
        QTest::keyClick(view, Qt::Key_Tab); // the canvas was laid out again: reach the LED
        QTest::keyClick(view, Qt::Key_Right);
        QTRY_VERIFY(partsOf(fake.saved, 0).contains(u"at"_s));
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QVERIFY(panel.tidy()); // once: QTRY_VERIFY would call it again
        QTRY_VERIFY2(!partsOf(fake.saved, 0).contains(u"at"_s), qPrintable(fake.saves.join(u" | "_s)));
        QTRY_VERIFY(!panel.isSaving()); // the core has answered

        // An added part can be undone too.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        // Off while a save is unanswered: wait for it, as a person does.
        QTRY_VERIFY(panel.undoButton()->isEnabled());
        panel.undoButton()->click();
        QTRY_COMPARE(panel.parts().size(), 1);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QVERIFY(!panel.redoButton()->isEnabled());

        // Redo makes again what Undo took back; a new change ends that.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        // Off while a save is unanswered: wait for it, as a person does.
        QTRY_VERIFY(panel.undoButton()->isEnabled());
        panel.undoButton()->click();
        QTRY_COMPARE(panel.parts().size(), 1);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QVERIFY(panel.redoButton()->isEnabled());
        // Off while a save is unanswered: wait for it, as a person does.
        QTRY_VERIFY(panel.redoButton()->isEnabled());
        panel.redoButton()->click();
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered

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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        const QJsonObject copy = panel.parts().at(1).toObject();
        QCOMPARE(copy.value(u"kind"_s).toString(), u"led"_s);
        QCOMPARE(copy.value(u"pins"_s).toObject().begin().value().toInt(), 5);
    }

    void copiesAndPastesParts()
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QVERIFY(!panel.copyPart()); // nothing selected
        // The board's pins to be careful with.
        QTRY_VERIFY(panel.pinNote(12).contains(u"strapping pin"_s));
        QVERIFY(panel.pinNote(34).contains(u"input only"_s));
        QVERIFY(panel.pinNote(4).isEmpty());
        // Hovering a board pin: its GPIO, the parts on it, its note.
        QTRY_VERIFY(panel.pinTip(2).contains(u"Wired to led-1"_s));
        QVERIFY(panel.pinTip(5).contains(u"No part on it"_s));
        // A circuit file: out, and in in place of the parts, which Undo puts back.
        panel.exportTo(u"/tmp/x/c.json"_s);
        QTRY_VERIFY(panel.statusText().contains(u"Exported the circuit to /tmp/x/c.json"_s));
        panel.importFrom(u"/tmp/x/bad.json"_s);
        QTRY_VERIFY(panel.statusText().contains(u"not a circuit file"_s));
        QVERIFY(!panel.undoButton()->isEnabled()); // nothing changed
        panel.importFrom(u"/tmp/x/c.json"_s);
        QTRY_COMPARE(panel.parts().at(0).toObject().value(u"id"_s).toString(), u"button-1"_s);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QTRY_VERIFY(panel.undoButton()->isEnabled());
        // Off while a save is unanswered: wait for it, as a person does.
        QTRY_VERIFY(panel.undoButton()->isEnabled());
        panel.undoButton()->click();
        QTRY_COMPARE(panel.parts().at(0).toObject().value(u"id"_s).toString(), u"led-1"_s);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QVERIFY(!panel.examplesButton()->isEnabled());
        // Examples: the library projects that use a part, on this board where they can be.
        QMenu *examples = panel.examplesMenu(u"led"_s, QJsonArray{
            QJsonObject{{u"id"_s, u"blink"_s}, {u"name"_s, u"Blink"_s}, {u"boards"_s, QJsonArray{u"esp32c3"_s, u"esp32"_s}}},
            QJsonObject{{u"id"_s, u"traffic"_s}, {u"name"_s, u"Traffic light"_s}, {u"boards"_s, QJsonArray{u"esp32s3"_s}}}});
        QCOMPARE(examples->actions().size(), 2);
        QCOMPARE(examples->actions().at(0)->text(), u"Blink"_s);
        QSignalSpy asked(&panel, &CircuitPanel::exampleRequested);
        examples->actions().at(0)->trigger();
        examples->actions().at(1)->trigger();
        QCOMPARE(asked.at(0).at(1).toString(), u"esp32"_s); // the board shown
        QCOMPARE(asked.at(1).at(1).toString(), u"esp32s3"_s); // its own board
        delete examples;
        QMenu *none = panel.examplesMenu(u"ws2812"_s, {});
        QCOMPARE(none->actions().size(), 1);
        QVERIFY(!none->actions().at(0)->isEnabled());
        delete none;
        // The circuit as a picture, as Copy picture copies it.
        const QImage picture = panel.picture();
        QVERIFY(!picture.isNull());
        QVERIFY(picture.width() > 100 && picture.height() > 100);

        panel.selectPart(u"led-1"_s, true);
        QTRY_COMPARE(panel.selectedPartId(), u"led-1"_s);
        QVERIFY(panel.copyPart());
        const QMimeData *data = QGuiApplication::clipboard()->mimeData();
        QVERIFY(data->hasFormat(QString::fromLatin1(CircuitPanel::PartMime)));
        QCOMPARE(QJsonDocument::fromJson(data->data(QString::fromLatin1(CircuitPanel::PartMime))).object().value(u"kind"_s),
                 QJsonValue(u"led"_s));
        QTRY_VERIFY(panel.pasteButton()->isEnabled());

        // Into another project: the Add dialog, on a pin of that project no part uses.
        panel.showBoard(u"other"_s, u"esp32"_s, QJsonArray{u"2"_s, u"13"_s});
        QTRY_COMPARE(panel.parts().size(), 1);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        int offered = -1;
        QTimer::singleShot(0, this, [&offered] {
            auto *dialog = qobject_cast<QDialog *>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            offered = dialog->findChildren<QSpinBox *>().first()->value();
            dialog->accept();
        });
        panel.pasteButton()->click();
        QCOMPARE(offered, 13);
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QCOMPARE(panel.parts().at(1).toObject().value(u"kind"_s).toString(), u"led"_s);

        // A part of a kind this app does not have is refused.
        auto *odd = new QMimeData;
        odd->setData(QString::fromLatin1(CircuitPanel::PartMime), R"({"kind":"flux-capacitor","id":"x"})");
        QGuiApplication::clipboard()->setMimeData(odd);
        QVERIFY(!panel.pastePart());
        QVERIFY(panel.statusText().contains(u"not one this version"_s));
        QGuiApplication::clipboard()->clear();
        QTRY_VERIFY(!panel.pasteButton()->isEnabled());
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QVERIFY(panel.item(u"led-1"_s));
        QVERIFY(panel.addPartButton()->isEnabled());

        // Adding saves through the core, with fresh ids.
        panel.addPart(u"button"_s, 4, true, u"Go"_s);
        QTRY_COMPARE(panel.parts().size(), 2);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QJsonObject b = partsOf(fake.saved, 1);
        QCOMPARE(b.value(u"id"_s).toString(), u"button-1"_s);
        QCOMPARE(b.value(u"pins"_s).toObject().value(u"pin"_s).toInt(), 4);
        QCOMPARE(b.value(u"props"_s).toObject().value(u"activeLow"_s).toBool(), true);
        QCOMPARE(b.value(u"label"_s).toString(), u"Go"_s);
        QCOMPARE(fake.saved.value(u"schemaVersion"_s).toString(), u"1.3.0"_s);
        panel.addPart(u"rgb-led"_s, QJsonObject{{u"red"_s, 25}, {u"green"_s, 26}, {u"blue"_s, 27}}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 3);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QCOMPARE(partsOf(fake.saved, 2).value(u"id"_s).toString(), u"rgb-led-1"_s);
        panel.addPart(u"switch"_s, QJsonObject{{u"pin"_s, 5}}, {}, {});
        QTRY_COMPARE(panel.parts().size(), 4);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        // An I2C part: no pins, a bus and an address.
        panel.addPart(u"ssd1306"_s, {}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 60}});
        QTRY_COMPARE(panel.parts().size(), 5);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QCOMPARE(partsOf(fake.saved, 4).value(u"i2c"_s).toObject().value(u"address"_s).toInt(), 60);
        QVERIFY(!partsOf(fake.saved, 4).contains(u"pins"_s));
        panel.addPart(u"bme280"_s, {}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 118}});
        QTRY_COMPARE(panel.parts().size(), 6);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        // An I2C part with a pin as well: it stays with the I2C parts, and names the pin.
        panel.addPart(u"ds3231"_s, QJsonObject{{u"sqw"_s, 25}}, {}, {}, QJsonObject{{u"bus"_s, 0}, {u"address"_s, 104}});
        QTRY_COMPARE(panel.parts().size(), 7);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
        QTRY_COMPARE(panel.partToolTip(u"ds3231-1"_s), u"I2C bus 0, address 0x68, SQW on GPIO 25"_s);
        panel.removePart(u"ds3231-1"_s);
        QTRY_COMPARE(panel.parts().size(), 6);
        QTRY_VERIFY(!panel.isSaving()); // the core has answered

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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
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
        QTRY_VERIFY(!panel.isSaving()); // the core has answered
    }
};

QTEST_MAIN(TestCircuit)
#include "tst_circuit.moc"
