// DisplayPanel and CoreConnection's sim.frame handling against a fake core.

#include "CoreConnection.h"
#include "DisplayPanel.h"

#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QPushButton>
#include <QClipboard>
#include <QGuiApplication>
#include <QImage>
#include <QTest>

using namespace Qt::StringLiterals;

class FakeCore : public QObject {
    Q_OBJECT
public:
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

    QJsonObject saved; // the last frame.save's params

    // A width x height picture: the left half red, the right half blue.
    void frame(int w, int h, const QString &format = u"rgb888"_s, int dropBytes = 0)
    {
        QByteArray rgb;
        for (int y = 0; y < h; ++y)
            for (int x = 0; x < w; ++x)
                rgb += x < w / 2 ? QByteArray("\xff\x00\x00", 3) : QByteArray("\x00\x00\xff", 3);
        rgb.chop(dropBytes);
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.frame"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"width"_s, w}, {u"height"_s, h},
                                        {u"format"_s, format}, {u"data"_s, QString::fromLatin1(rgb.toBase64())}}}});
    }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        if (m == u"core.hello")
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.35.0"_s}});
        else if (m == u"frame.save") {
            saved = req.value(u"params"_s).toObject();
            reply(id, QJsonObject{{u"path"_s, saved.value(u"path"_s)}, {u"bytes"_s, 1}});
        } else if (m == u"sim.start")
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, false}, {u"engine"_s, u"lcgamboa"_s}});
        else
            reply(id, QJsonObject{});
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestDisplay : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void showsFramesSharplyAndOnlyFromItsSimulator()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        DisplayPanel panel(&core);
        panel.resize(700, 600);
        panel.show();
        QVERIFY(QTest::qWaitForWindowExposed(&panel));
        QSignalSpy first(&panel, &DisplayPanel::firstFrame);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        QVERIFY(panel.image().isNull());

        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        fake.frame(320, 240);
        QTRY_COMPARE(panel.image().size(), QSize(320, 240));
        QCOMPARE(panel.image().pixelColor(10, 10), QColor(255, 0, 0));
        QCOMPARE(panel.image().pixelColor(300, 200), QColor(0, 0, 255));
        QVERIFY(panel.statusText().contains(u"320"_s));
        QCOMPARE(first.size(), 1);
        // Room for twice the size: scaled by a whole number.
        QCOMPARE(panel.scale(), 2);

        // A fixed zoom, and back to fitting.
        panel.setZoom(3);
        QCOMPARE(panel.scale(), 3);
        panel.setZoom(0);
        QCOMPARE(panel.scale(), 2);
        // Save picture… hands the pixels to the core, rows of red, green and blue.
        QVERIFY(panel.saveButton()->isEnabled());
        panel.saveTo(u"/tmp/panel.png"_s);
        QTRY_COMPARE(fake.saved.value(u"width"_s).toInt(), 320);
        const QByteArray rgb = QByteArray::fromBase64(fake.saved.value(u"data"_s).toString().toLatin1());
        QCOMPARE(rgb.size(), 320 * 240 * 3);
        QCOMPARE(rgb.left(3), QByteArray("\xff\x00\x00", 3));
        QTRY_VERIFY(panel.statusText().contains(u"/tmp/panel.png"_s));
        // Another dock's picture saved through the same call is not claimed here.
        core.saveFrame(u"/tmp/plot.png"_s, QImage(8, 8, QImage::Format_RGB888));
        QTest::qWait(100);
        QVERIFY(!panel.statusText().contains(u"/tmp/plot.png"_s));
        // Copy picture: the picture as it is, on the clipboard.
        QVERIFY(panel.copyButton()->isEnabled());
        panel.copyButton()->click();
        QCOMPARE(QGuiApplication::clipboard()->image().size(), QSize(320, 240));

        // Bad frames are ignored rather than drawn wrong.
        fake.frame(4, 4, u"rgb565"_s);
        fake.frame(4, 4, u"rgb888"_s, 3);
        fake.frame(160, 120);
        QTRY_COMPARE(panel.image().size(), QSize(160, 120));
        QCOMPARE(first.size(), 1); // still the same session

        core.stopSim();
        QTRY_VERIFY(panel.statusText().contains(u"stopped"_s));
        QCOMPARE(panel.image().size(), QSize(160, 120)); // the last picture stays

        // A new session starts clean and announces its first picture again.
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        QVERIFY(panel.image().isNull());
        fake.frame(320, 240);
        QTRY_COMPARE(first.size(), 2);

        // A run that never draws says so when it stops.
        core.stopSim();
        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_VERIFY(core.isSimRunning());
        core.stopSim();
        QTRY_VERIFY(panel.statusText().contains(u"did not draw"_s));
    }
};

QTEST_MAIN(TestDisplay)
#include "tst_display.moc"
