// JSON-RPC 2.0 client for the esp32bench core, over a local socket with one JSON
// object per line (docs/rpc.md). All calls are asynchronous.

#pragma once

#include <QHash>
#include <QJsonObject>
#include <QJsonValue>
#include <QLocalSocket>
#include <QObject>
#include <QPointer>

#include <functional>

// The outcome of one call: a result, or an error code and message.
struct RpcReply {
    bool ok = false;
    QJsonValue result;
    int errorCode = 0;
    QString errorMessage;
    QJsonValue errorData; // the error's `data`, e.g. a consent token and plan
};

class RpcClient : public QObject {
    Q_OBJECT

public:
    // Error code used locally when the connection drops before a reply arrives.
    static constexpr int DisconnectedError = -1;

    explicit RpcClient(QObject *parent = nullptr);

    void connectToCore(const QString &socketPath);
    void disconnectFromCore();
    bool isConnected() const;

    // Send a request. `handler` runs later on the GUI thread with the reply, but only
    // if `context` still exists, so a closed widget never receives a stale reply.
    void call(const QString &method, const QJsonObject &params, QObject *context,
              std::function<void(const RpcReply &)> handler);

signals:
    void connected();
    void disconnected();
    // A socket-level error, e.g. the core is not running.
    void connectionError(const QString &message);
    // A notification from the core (an event on a subscription).
    void notification(const QString &method, const QJsonObject &params);

private:
    struct Pending {
        QPointer<QObject> context;
        std::function<void(const RpcReply &)> handler;
    };

    void onReadyRead();
    void handleMessage(const QJsonObject &msg);
    void failAllPending(const QString &message);

    QLocalSocket *m_socket;
    QHash<qint64, Pending> m_pending;
    qint64 m_nextId = 1;
};
