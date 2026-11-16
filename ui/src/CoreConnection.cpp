#include "CoreConnection.h"

#include "RpcClient.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QJsonObject>
#include <QProcess>
#include <QStandardPaths>
#include <QTimer>

using namespace Qt::StringLiterals;

namespace {
constexpr int RetryMs = 2000;
}

CoreConnection::CoreConnection(QObject *parent)
    : QObject(parent)
    , m_client(new RpcClient(this))
    , m_retry(new QTimer(this))
    , m_socketPath(defaultSocketPath())
{
    m_retry->setInterval(RetryMs);
    m_retry->setSingleShot(true);
    connect(m_retry, &QTimer::timeout, this, &CoreConnection::tryConnect);

    connect(m_client, &RpcClient::connected, this, &CoreConnection::onSocketConnected);
    connect(m_client, &RpcClient::notification, this, &CoreConnection::onNotification);
    connect(m_client, &RpcClient::disconnected, this, [this] {
        m_devicesSubscription = 0; // the core drops subscriptions with the connection
        // After an incompatible handshake we hang up on purpose; stay in that state
        // instead of retrying the same core forever.
        if (m_state == State::Incompatible)
            return;
        setState(State::Disconnected);
        m_retry->start();
    });
    connect(m_client, &RpcClient::connectionError, this, [this](const QString &message) {
        m_lastError = message;
        if (m_state != State::Incompatible) {
            setState(State::Disconnected);
            m_retry->start();
        }
    });
}

QString CoreConnection::defaultSocketPath()
{
    const QString env = qEnvironmentVariable("ESP32BENCH_SOCKET");
    if (!env.isEmpty())
        return env;
    const QString runtime = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
    return QDir(runtime).filePath(u"esp32bench/core.sock"_s);
}

void CoreConnection::setSocketPath(const QString &path)
{
    m_socketPath = path;
}

void CoreConnection::start()
{
    tryConnect();
}

void CoreConnection::tryConnect()
{
    if (m_state == State::Connected || m_state == State::Incompatible)
        return;
    m_client->connectToCore(m_socketPath);
}

void CoreConnection::onSocketConnected()
{
    setState(State::Connecting);
    const QJsonObject params{
        {u"client"_s, u"esp32bench-ui %1"_s.arg(QStringLiteral(ESP32BENCH_UI_VERSION))},
        {u"protocol"_s, u"0.2.0"_s},
    };
    m_client->call(u"core.hello"_s, params, this, [this](const RpcReply &r) {
        if (!r.ok) {
            m_lastError = r.errorMessage;
            m_client->disconnectFromCore();
            return;
        }
        const QJsonObject o = r.result.toObject();
        m_coreVersion = o.value(u"version"_s).toString();
        const QString protocol = o.value(u"protocol"_s).toString();
        const int major = protocol.section(u'.', 0, 0).toInt();
        if (protocol.isEmpty() || major != ProtocolMajor) {
            m_lastError = tr("The core speaks protocol %1; this program needs %2.x.")
                              .arg(protocol)
                              .arg(ProtocolMajor);
            setState(State::Incompatible);
            m_client->disconnectFromCore();
            return;
        }
        m_lastError.clear();
        setState(State::Connected);
    });
}

void CoreConnection::setState(State s)
{
    if (s == m_state)
        return;
    m_state = s;
    emit stateChanged(s);
}

bool CoreConnection::startCore()
{
    QString program = qEnvironmentVariable("ESP32BENCH_DAEMON");
    if (program.isEmpty()) {
        const QString beside =
            QDir(QCoreApplication::applicationDirPath()).filePath(u"esp32benchd"_s);
        program = QFileInfo(beside).isExecutable()
                      ? beside
                      : QStandardPaths::findExecutable(u"esp32benchd"_s);
    }
    if (program.isEmpty()) {
        m_lastError = tr("Could not find the esp32benchd program.");
        return false;
    }
    QStringList args;
    if (m_socketPath != defaultSocketPath())
        args << u"--socket"_s << m_socketPath;
    // Detached so the core outlives the UI; no inherited stdio, so it never holds
    // the launching terminal's pipes open.
    QProcess process;
    process.setProgram(program);
    process.setArguments(args);
    process.setStandardInputFile(QProcess::nullDevice());
    process.setStandardOutputFile(QProcess::nullDevice());
    process.setStandardErrorFile(QProcess::nullDevice());
    qint64 pid = 0;
    if (!process.startDetached(&pid)) {
        m_lastError = tr("Could not start %1.").arg(program);
        return false;
    }
    m_startedCorePid = pid;
    // Give it a moment to bind its socket, then connect.
    QTimer::singleShot(300, this, &CoreConnection::tryConnect);
    return true;
}

void CoreConnection::listDevices(bool includeNonUsb)
{
    const QString method = u"device.list"_s;
    m_client->call(method, {{u"all"_s, includeNonUsb}}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            emit devicesListed(r.result.toObject().value(u"ports"_s).toArray());
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::watchDevices()
{
    const QString method = u"device.watch"_s;
    m_client->call(method, {}, this, [this, method](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(method, r.errorMessage);
            return;
        }
        const QJsonObject o = r.result.toObject();
        m_devicesSubscription = o.value(u"subscription"_s).toInteger();
        emit devicesListed(o.value(u"ports"_s).toArray());
    });
}

void CoreConnection::onNotification(const QString &method, const QJsonObject &params)
{
    const qint64 sub = params.value(u"subscription"_s).toInteger();
    if (method == u"device.changed" && sub != 0 && sub == m_devicesSubscription)
        emit devicesListed(params.value(u"ports"_s).toArray());
}
