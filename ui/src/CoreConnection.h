// The UI's link to the core daemon: connects, checks the protocol with core.hello,
// reconnects when the core goes away, and can start esp32benchd. Widgets use the
// typed request methods here and listen for the result signals; they never build
// JSON-RPC requests themselves.

#pragma once

#include <QJsonArray>
#include <QObject>
#include <QString>

class QTimer;
class RpcClient;

class CoreConnection : public QObject {
    Q_OBJECT

public:
    enum class State {
        Disconnected,  // No core reachable; retrying in the background.
        Connecting,    // Socket open, waiting for core.hello.
        Connected,     // Handshake done; requests can be sent.
        Incompatible,  // The core speaks a protocol this UI does not understand.
    };
    Q_ENUM(State)

    // Protocol major version this UI understands (schema/rpc/index.json).
    static constexpr int ProtocolMajor = 0;

    explicit CoreConnection(QObject *parent = nullptr);

    // Socket path: $ESP32BENCH_SOCKET, else $XDG_RUNTIME_DIR/esp32bench/core.sock.
    static QString defaultSocketPath();

    void setSocketPath(const QString &path);
    QString socketPath() const { return m_socketPath; }

    void start();          // Begin connecting, and keep retrying while disconnected.
    State state() const { return m_state; }
    QString coreVersion() const { return m_coreVersion; }
    QString lastError() const { return m_lastError; }

    // Launch esp32benchd detached, so it outlives the UI. Returns false if no daemon
    // binary was found ($ESP32BENCH_DAEMON, next to this program, or on PATH).
    bool startCore();
    // Process id of the core this object last started, or 0.
    qint64 startedCorePid() const { return m_startedCorePid; }

    // device.list
    void listDevices(bool includeNonUsb = false);

    RpcClient *client() const { return m_client; }

signals:
    void stateChanged(CoreConnection::State state);
    void devicesListed(const QJsonArray &ports);
    // A request failed. `method` is the RPC method name.
    void requestFailed(const QString &method, const QString &message);

private:
    void setState(State s);
    void tryConnect();
    void onSocketConnected();

    RpcClient *m_client;
    QTimer *m_retry;
    QString m_socketPath;
    QString m_coreVersion;
    QString m_lastError;
    State m_state = State::Disconnected;
    qint64 m_startedCorePid = 0;
};
