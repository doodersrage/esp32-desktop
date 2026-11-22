// PinsPanel and CoreConnection's pin calls against a fake core.

#include "CoreConnection.h"
#include "PinsPanel.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalServer>
#include <QLocalSocket>
#include <QPointer>
#include <QPushButton>
#include <QTemporaryDir>
#include <QLocale>
#include <QSignalSpy>
#include <QTest>
#include <QTreeWidget>

using namespace Qt::StringLiterals;

class FakeCore : public QObject {
    Q_OBJECT
public:
    QJsonArray pinsToReport{2, 4};
    QList<QJsonObject> setPins; // sim.setPin params

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

    void pins(const QJsonArray &changed)
    {
        send({{u"jsonrpc"_s, u"2.0"_s}, {u"method"_s, u"sim.pins"_s},
              {u"params"_s, QJsonObject{{u"subscription"_s, 5}, {u"pins"_s, changed}}}});
    }

private:
    void send(const QJsonObject &o) { m_sock->write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n'); }
    void reply(const QJsonValue &id, const QJsonValue &r) { send({{u"jsonrpc"_s, u"2.0"_s}, {u"id"_s, id}, {u"result"_s, r}}); }
    void handle(const QJsonObject &req)
    {
        const QJsonValue id = req.value(u"id"_s);
        const QString m = req.value(u"method"_s).toString();
        if (m == u"core.hello") {
            reply(id, QJsonObject{{u"name"_s, u"esp32benchd"_s}, {u"version"_s, u"0.0.1"_s}, {u"protocol"_s, u"0.36.0"_s}});
        } else if (m == u"sim.start") {
            reply(id, QJsonObject{{u"subscription"_s, 5}, {u"chip"_s, u"esp32"_s}, {u"debug"_s, false},
                                  {u"engine"_s, u"lcgamboa"_s}, {u"pins"_s, pinsToReport}});
        } else if (m == u"sim.setPin") {
            setPins << req.value(u"params"_s).toObject();
            reply(id, QJsonObject{});
        } else {
            reply(id, QJsonObject{});
        }
    }
    QLocalServer *m_server;
    QPointer<QLocalSocket> m_sock;
};

class TestPins : public QObject {
    Q_OBJECT

private:
    QTemporaryDir m_dir;
    QString path() const { return m_dir.filePath(u"core.sock"_s); }

private slots:
    void showsAndDrivesTheProjectsPins()
    {
        FakeCore fake(path());
        CoreConnection core;
        core.setSocketPath(path());
        PinsPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);

        core.startSim(u"blink"_s, u"esp32"_s);
        QTRY_COMPARE(panel.tree()->topLevelItemCount(), 2);
        QCOMPARE(core.simPins(), (QList<int>{2, 4}));
        QCOMPARE(panel.row(2)->text(PinsPanel::LevelColumn), u"unknown"_s);

        fake.pins(QJsonArray{QJsonObject{{u"gpio"_s, 2}, {u"level"_s, true}, {u"output"_s, true}}});
        QTRY_COMPARE(panel.row(2)->text(PinsPanel::LevelColumn), u"high"_s);
        QCOMPARE(panel.row(2)->text(PinsPanel::DirectionColumn), u"output"_s);
        fake.pins(QJsonArray{QJsonObject{{u"gpio"_s, 2}, {u"level"_s, false}},
                             QJsonObject{{u"gpio"_s, 4}, {u"output"_s, false}},
                             QJsonObject{{u"gpio"_s, 9}, {u"level"_s, true}}}); // not ours: ignored
        QTRY_COMPARE(panel.row(2)->text(PinsPanel::LevelColumn), u"low"_s);
        QCOMPARE(panel.row(4)->text(PinsPanel::DirectionColumn), u"input"_s);
        QVERIFY(!panel.row(9));
        // A double-click on a pin asks to see it in the trace.
        QSignalSpy activated(&panel, &PinsPanel::pinActivated);
        emit panel.tree()->itemActivated(panel.row(4), 0);
        QCOMPARE(activated.size(), 1);
        QCOMPARE(activated.at(0).at(0).toInt(), 4);
        // How fast a pin toggles, and when it stops.
        QVERIFY(panel.row(2)->text(PinsPanel::RateColumn).isEmpty());
        fake.pins(QJsonArray{QJsonObject{{u"gpio"_s, 2}, {u"rate"_s, QJsonObject{{u"hz"_s, 1.0}, {u"high"_s, 0.5}}}}});
        QTRY_COMPARE(panel.row(2)->text(PinsPanel::RateColumn),
                     u"%1 Hz · 50% high"_s.arg(QLocale().toString(1.0, 'f', 2)));
        fake.pins(QJsonArray{QJsonObject{{u"gpio"_s, 2}, {u"rate"_s, QJsonValue::Null}}});
        QTRY_VERIFY(panel.row(2)->text(PinsPanel::RateColumn).isEmpty());
        QCOMPARE(PinsPanel::rateText(QJsonObject{{u"hz"_s, 5000.0}, {u"high"_s, 0.25}}),
                 u"%1 kHz · 25% high"_s.arg(QLocale().toString(5.0, 'f', 2)));

        auto *drive = qobject_cast<QPushButton *>(panel.tree()->itemWidget(panel.row(4), PinsPanel::DriveColumn));
        QVERIFY(drive);
        drive->click();
        QTRY_COMPARE(fake.setPins.size(), 1);
        QCOMPARE(fake.setPins[0].value(u"gpio"_s).toInt(), 4);
        QCOMPARE(fake.setPins[0].value(u"level"_s).toBool(), true);
        QCOMPARE(drive->text(), u"High"_s);
        drive->click();
        QTRY_COMPARE(fake.setPins.size(), 2);
        QCOMPARE(fake.setPins[1].value(u"level"_s).toBool(), false);

        core.stopSim();
        QTRY_VERIFY(panel.statusText().contains(u"stopped"_s));
        QVERIFY(!drive->isEnabled());
        QVERIFY(core.simPins().isEmpty());
        // Nothing is sent for a stopped simulation.
        core.setSimPin(4, true);
        QTest::qWait(100);
        QCOMPARE(fake.setPins.size(), 2);
    }

    void saysWhyThereAreNoPins()
    {
        FakeCore fake(path());
        fake.pinsToReport = {};
        CoreConnection core;
        core.setSocketPath(path());
        PinsPanel panel(&core);
        core.start();
        QTRY_COMPARE(core.state(), CoreConnection::State::Connected);
        core.startSim(u"blink"_s, u"esp32s3"_s);
        QTRY_VERIFY(panel.statusText().contains(u"No pin access"_s));
        QCOMPARE(panel.tree()->topLevelItemCount(), 0);
    }
};

QTEST_MAIN(TestPins)
#include "tst_pins.moc"
