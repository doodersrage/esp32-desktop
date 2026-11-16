#include "RpcClient.h"

#include <QJsonDocument>

using namespace Qt::StringLiterals;

namespace {
// Same limit as the core (docs/rpc.md).
constexpr qint64 MaxMessage = 16 * 1024 * 1024;
}

RpcClient::RpcClient(QObject *parent)
    : QObject(parent)
    , m_socket(new QLocalSocket(this))
{
    connect(m_socket, &QLocalSocket::connected, this, &RpcClient::connected);
    connect(m_socket, &QLocalSocket::disconnected, this, [this] {
        failAllPending(tr("The connection to the core was lost."));
        emit disconnected();
    });
    connect(m_socket, &QLocalSocket::errorOccurred, this, [this](QLocalSocket::LocalSocketError) {
        emit connectionError(m_socket->errorString());
    });
    connect(m_socket, &QLocalSocket::readyRead, this, &RpcClient::onReadyRead);
}

void RpcClient::connectToCore(const QString &socketPath)
{
    if (m_socket->state() != QLocalSocket::UnconnectedState)
        m_socket->abort();
    m_socket->connectToServer(socketPath);
}

void RpcClient::disconnectFromCore()
{
    m_socket->disconnectFromServer();
}

bool RpcClient::isConnected() const
{
    return m_socket->state() == QLocalSocket::ConnectedState;
}

void RpcClient::call(const QString &method, const QJsonObject &params, QObject *context,
                     std::function<void(const RpcReply &)> handler)
{
    if (!isConnected()) {
        // Answer asynchronously, like a real reply, so callers have one code path.
        QMetaObject::invokeMethod(this, [context = QPointer<QObject>(context), handler, this] {
            if (context && handler)
                handler({false, {}, DisconnectedError, tr("Not connected to the core.")});
        }, Qt::QueuedConnection);
        return;
    }
    const qint64 id = m_nextId++;
    m_pending.insert(id, {context, std::move(handler)});
    const QJsonObject req{
        {u"jsonrpc"_s, u"2.0"_s},
        {u"id"_s, id},
        {u"method"_s, method},
        {u"params"_s, params},
    };
    m_socket->write(QJsonDocument(req).toJson(QJsonDocument::Compact) + '\n');
}

void RpcClient::onReadyRead()
{
    while (m_socket->canReadLine()) {
        const QByteArray line = m_socket->readLine().trimmed();
        if (line.isEmpty())
            continue;
        QJsonParseError err;
        const QJsonDocument doc = QJsonDocument::fromJson(line, &err);
        if (err.error != QJsonParseError::NoError || !doc.isObject())
            continue; // The core only sends objects; ignore anything else.
        handleMessage(doc.object());
    }
    if (m_socket->bytesAvailable() > MaxMessage) {
        emit connectionError(tr("The core sent a message that is too long."));
        m_socket->abort();
    }
}

void RpcClient::handleMessage(const QJsonObject &msg)
{
    if (!msg.contains(u"id"_s)) {
        const QString method = msg.value(u"method"_s).toString();
        if (!method.isEmpty())
            emit notification(method, msg.value(u"params"_s).toObject());
        return;
    }
    const qint64 id = msg.value(u"id"_s).toInteger(-1);
    const auto it = m_pending.find(id);
    if (it == m_pending.end())
        return;
    const Pending p = it.value();
    m_pending.erase(it);
    if (!p.context || !p.handler)
        return;

    RpcReply reply;
    if (msg.contains(u"result"_s)) {
        reply.ok = true;
        reply.result = msg.value(u"result"_s);
    } else {
        const QJsonObject e = msg.value(u"error"_s).toObject();
        reply.errorCode = e.value(u"code"_s).toInt();
        reply.errorMessage = e.value(u"message"_s).toString();
    }
    p.handler(reply);
}

void RpcClient::failAllPending(const QString &message)
{
    const auto pending = std::exchange(m_pending, {});
    for (const Pending &p : pending) {
        if (p.context && p.handler)
            p.handler({false, {}, DisconnectedError, message});
    }
}
