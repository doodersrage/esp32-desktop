#include "CoreConnection.h"
#include "Preferences.h"

#include "RpcClient.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QJsonObject>
#include <QProcess>

#if defined(Q_OS_WIN)
#include <windows.h>
#endif
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
        m_devicesSubscription = 0; // the core drops subscriptions and jobs with the connection
        m_earlyLibraryEvents.clear();
        m_earlyBuildEvents.clear();
        m_earlySerialEvents.clear();
        m_earlyInstallEvents.clear();
        m_earlyBoardEvents.clear();
        m_earlySimEvents.clear();
        m_traceSub = 0; // the core keeps traces per connection
        if (m_simSub != 0) {
            m_simSub = 0;
            m_simPaused = false;
            m_simPins.clear();
            m_simPartsActive = false;
            emit simStopped(tr("The connection to the core was lost."));
        }
        if (m_boardJob != 0) {
            m_boardJob = 0;
            emit boardFinished(m_boardOperation, false, tr("The connection to the core was lost."), {});
        }
        if (m_installJob != 0) {
            m_installJob = 0;
            emit toolchainInstallFinished(false, tr("The connection to the core was lost."));
        }
        if (m_serialSub != 0) {
            m_serialSub = 0;
            emit serialClosed(tr("The connection to the core was lost."));
        }
        if (m_buildJob != 0) {
            m_buildJob = 0;
            emit buildFinished(false, tr("The connection to the core was lost."), {});
        }
        if (m_libraryJob != 0) {
            m_libraryJob = 0;
            emit libraryRefreshed(false, tr("The connection to the core was lost."));
        }
        // After an incompatible handshake we hang up on purpose; stay in that state
        // instead of retrying the same core forever.
        if (m_state == State::Incompatible)
            return;
        setState(State::Disconnected);
        m_retry->start();
    });
    connect(m_client, &RpcClient::connectionError, this, [this](const QString &message) {
        m_lastError = message;
        if (m_autoStart && !m_autoStartTried && m_state != State::Incompatible) {
            m_autoStartTried = true;
            if (startCore())
                return; // it connects once the core has bound its socket
            emit autoStartFailed(m_lastError);
        }
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
    // As the core's esp32bench_rpc::default_socket_path: $XDG_RUNTIME_DIR on Linux,
    // $TMPDIR on macOS (per user; QDir::tempPath reads it), and on Windows the named
    // pipe esp32bench-<user>-core (QLocalSocket adds \\.\pipe\).
#if defined(Q_OS_WIN)
    QString user = qEnvironmentVariable("USERNAME");
    for (QChar &c : user) {
        if (!(c.isLetterOrNumber() && c.unicode() < 128))
            c = u'_';
    }
    return u"esp32bench-%1-core"_s.arg(user);
#elif defined(Q_OS_MACOS)
    const QString runtime = QDir::tempPath();
#else
    const QString runtime = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
#endif
#if !defined(Q_OS_WIN)
    return QDir(runtime).filePath(u"esp32bench/core.sock"_s);
#endif
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
        {u"protocol"_s, u"%1.%2.0"_s.arg(ProtocolMajor).arg(ProtocolMinor)},
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
        // Same major but older: a core left running from an earlier version (after
        // an upgrade, say). It would refuse what is new, so say how to replace it.
        if (protocol.section(u'.', 1, 1).toInt() < ProtocolMinor) {
            m_lastError = tr("an older core (protocol %1) is still running; this version needs %2.%3 or newer. "
                             "Stop it (pkill -x esp32benchd), then press Start core.")
                              .arg(protocol)
                              .arg(ProtocolMajor)
                              .arg(ProtocolMinor);
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
#if defined(Q_OS_WIN)
        const QString name = u"esp32benchd.exe"_s;
#else
        const QString name = u"esp32benchd"_s;
#endif
        const QString beside = QDir(QCoreApplication::applicationDirPath()).filePath(name);
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
#if defined(Q_OS_WIN)
    // The core is a console program: run it without a console window. The simulator
    // processes it starts share its hidden console.
    process.setCreateProcessArgumentsModifier([](QProcess::CreateProcessArguments *a) {
        a->flags |= CREATE_NO_WINDOW;
    });
#endif
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
        if (r.ok) {
            const QJsonObject o = r.result.toObject();
            emit deviceAdvice(o.value(u"advice"_s).toArray());
            emit devicesListed(o.value(u"ports"_s).toArray());
        } else {
            emit requestFailed(method, r.errorMessage);
        }
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
        emit deviceAdvice(o.value(u"advice"_s).toArray());
        emit devicesListed(o.value(u"ports"_s).toArray());
    });
}

void CoreConnection::onNotification(const QString &method, const QJsonObject &params)
{
    const qint64 sub = params.value(u"subscription"_s).toInteger();
    if (sub == 0)
        return;
    if (method == u"device.changed" && sub == m_devicesSubscription) {
        emit deviceAdvice(params.value(u"advice"_s).toArray());
        emit devicesListed(params.value(u"ports"_s).toArray());
    } else if (method == u"build.log" || method == u"build.finished") {
        if (sub == m_buildJob)
            handleBuildEvent(method, params);
        else if (m_buildJob == -1)
            m_earlyBuildEvents.append({method, params}); // our job id is not known yet
    } else if (method == u"sim.recorded" && sub == m_lastSimSub && m_simSub != sub) {
        // A recording written as the simulation ended, after it said so.
        handleSimEvent(method, params);
    } else if (method == u"sim.output" || method == u"sim.frame" || method == u"sim.pins" || method == u"sim.trace" || method == u"sim.parts" || method == u"sim.partPicture"
               || method == u"sim.exited" || method == u"sim.wifiJoined" || method == u"sim.expect" || method == u"sim.clock"
               || method == u"sim.paused" || method == u"sim.backtrace" || method == u"sim.bootLoop"
               || method == u"sim.recorded"
               || method.startsWith(u"debug.")) {
        if (sub == m_simSub)
            handleSimEvent(method, params);
        else if (m_simSub == -1)
            m_earlySimEvents.append({method, params}); // our id is not known yet
    } else if (method == u"board.progress" || method == u"board.finished") {
        if (sub == m_boardJob)
            handleBoardEvent(method, params);
        else if (m_boardJob == -1)
            m_earlyBoardEvents.append({method, params}); // our job id is not known yet
    } else if (method == u"toolchain.progress" || method == u"toolchain.finished"
               || method == u"toolchain.download") {
        if (sub == m_installJob)
            handleInstallEvent(method, params);
        else if (m_installJob == -1)
            m_earlyInstallEvents.append({method, params}); // our job id is not known yet
    } else if (method == u"serial.data" || method == u"serial.closed" || method == u"serial.lost"
               || method == u"serial.reconnected" || method == u"serial.expect" || method == u"serial.garbled"
               || method == u"serial.recordFailed" || method == u"serial.backtrace"
               || method == u"serial.bootLoop") {
        if (sub == m_serialSub)
            handleSerialEvent(method, params);
        else if (m_serialSub == -1)
            m_earlySerialEvents.append({method, params}); // our id is not known yet
    } else if (method == u"library.refreshed") {
        if (sub == m_libraryJob)
            finishLibraryRefresh(params);
        else if (m_libraryJob == -1)
            m_earlyLibraryEvents << params; // our job id is not known yet
    }
}

void CoreConnection::listLibrary()
{
    const QString method = u"library.list"_s;
    m_client->call(method, {}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            emit libraryListed(r.result.toObject());
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::starProject(const QString &id, bool starred)
{
    const QString method = u"library.star"_s;
    m_client->call(method, {{u"id"_s, id}, {u"starred"_s, starred}}, this,
                   [this, method](const RpcReply &r) {
                       if (r.ok)
                           listLibrary();
                       else
                           emit requestFailed(method, r.errorMessage);
                   });
}

void CoreConnection::refreshLibrary()
{
    if (m_libraryJob != 0)
        return; // one at a time
    const QString method = u"library.refresh"_s;
    m_libraryJob = -1; // pending: set before the reply so a second click is ignored
    m_client->call(method, {}, this, [this, method](const RpcReply &r) {
        if (!r.ok) {
            m_libraryJob = 0;
            emit libraryRefreshed(false, r.errorMessage);
            return;
        }
        m_libraryJob = r.result.toObject().value(u"job"_s).toInteger();
        const auto early = std::exchange(m_earlyLibraryEvents, {});
        for (const QJsonObject &e : early) {
            if (e.value(u"subscription"_s).toInteger() == m_libraryJob) {
                finishLibraryRefresh(e);
                break;
            }
        }
    });
}

void CoreConnection::finishLibraryRefresh(const QJsonObject &params)
{
    m_libraryJob = 0;
    const bool ok = params.value(u"ok"_s).toBool();
    emit libraryRefreshed(ok, params.value(u"error"_s).toString());
    if (ok)
        listLibrary();
}

void CoreConnection::openLocal(const QString &path)
{
    const QString method = u"local.open"_s;
    m_client->call(method, {{u"path"_s, path}}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            emit localOpened(r.result.toObject().value(u"project"_s).toObject());
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::listLocal()
{
    const QString method = u"local.list"_s;
    m_client->call(method, {}, this, [this, method](const RpcReply &r) {
        if (r.ok) {
            const QJsonObject o = r.result.toObject();
            emit localListed(o.value(u"projects"_s).toArray(), o.value(u"idf"_s).toObject());
        } else {
            emit requestFailed(method, r.errorMessage);
        }
    });
}

void CoreConnection::getLocal(const QString &id)
{
    const QString method = u"local.get"_s;
    m_client->call(method, {{u"id"_s, id}}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            emit localLoaded(r.result.toObject());
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::removeLocal(const QString &id)
{
    const QString method = u"local.remove"_s;
    m_client->call(method, {{u"id"_s, id}}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            listLocal();
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::getProject(const QString &id)
{
    const QString method = u"library.get"_s;
    m_client->call(method, {{u"id"_s, id}}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            emit projectLoaded(r.result.toObject());
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::openLink(const QString &url)
{
    const QString method = u"link.open"_s;
    m_client->call(method, {{u"url"_s, url}}, this, [this, method](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(method, r.errorMessage);
            return;
        }
        const QJsonObject o = r.result.toObject();
        emit linkOpened(o.value(u"project"_s).toString(), o.value(u"board"_s).toString());
    });
}

void CoreConnection::startBuild(const QString &project, const QString &board, const QString &consent)
{
    if (m_buildJob != 0)
        return; // one build at a time
    QJsonObject params{{u"project"_s, project}, {u"board"_s, board}};
    if (!consent.isEmpty())
        params[u"consent"_s] = consent;
    m_buildJob = -1;
    m_client->call(u"build.start"_s, params, this, [this, project, board](const RpcReply &r) {
        if (!r.ok) {
            m_buildJob = 0;
            m_earlyBuildEvents.clear();
            const QJsonObject data = r.errorData.toObject();
            if (r.errorCode == ConsentRequired && data.contains(u"token"_s))
                emit buildConsentRequired(project, board, data.value(u"token"_s).toString(),
                                          data.value(u"plan"_s).toObject());
            else
                emit buildFinished(false, r.errorMessage, {});
            return;
        }
        const QJsonObject o = r.result.toObject();
        m_buildJob = o.value(u"job"_s).toInteger();
        emit buildStarted(o.value(u"plan"_s).toObject());
        // Events that beat the reply, in the order they came.
        const auto early = std::exchange(m_earlyBuildEvents, {});
        for (const auto &[method, params] : early) {
            if (m_buildJob > 0 && params.value(u"subscription"_s).toInteger() == m_buildJob)
                handleBuildEvent(method, params);
        }
    });
}

void CoreConnection::handleBuildEvent(const QString &method, const QJsonObject &params)
{
    if (method == u"build.log") {
        emit buildLog(params.value(u"stream"_s).toString(), params.value(u"line"_s).toString(),
                      params.value(u"level"_s).toString(), params.value(u"location"_s).toObject());
        if (params.contains(u"explain"_s))
            emit outputExplained(u"build"_s, params.value(u"explain"_s).toArray());
        return;
    }
    m_buildJob = 0;
    if (params.contains(u"firstError"_s))
        emit buildFirstError(params.value(u"firstError"_s).toObject());
    if (params.contains(u"size"_s))
        emit buildSize(params.value(u"size"_s).toObject());
    if (params.contains(u"seconds"_s))
        emit buildTime(params.value(u"seconds"_s).toDouble());
    emit buildFinished(params.value(u"ok"_s).toBool(), params.value(u"error"_s).toString(),
                       params.value(u"image"_s).toString());
}

void CoreConnection::cancelBuild()
{
    if (m_buildJob <= 0)
        return;
    const qint64 job = m_buildJob;
    m_client->call(u"job.cancel"_s, {{u"job"_s, job}}, this, [this, job](const RpcReply &r) {
        // A cancelled job sends no build.finished, so report it here.
        if (r.ok && m_buildJob == job) {
            m_buildJob = 0;
            emit buildFinished(false, tr("Cancelled."), {});
        }
    });
}

void CoreConnection::openSerial(const QString &path, int baud, const QString &expect, const QString &decodeBoard)
{
    if (m_serialSub != 0)
        return; // one port at a time
    m_serialSub = -1;
    // reconnect: a board that resets over its own USB, or is plugged back in, is
    // opened again by the core (serial.lost, then serial.reconnected).
    QJsonObject params{{u"path"_s, path}, {u"baud"_s, baud}, {u"reconnect"_s, Preferences::serialReconnect()}};
    if (!expect.isEmpty())
        params.insert(u"expect"_s, expect);
    if (!expect.isEmpty() && !decodeBoard.isEmpty())
        params.insert(u"decode"_s, QJsonObject{{u"project"_s, expect}, {u"board"_s, decodeBoard}});
    m_client->call(u"serial.open"_s, params, this,
                   [this, path, baud](const RpcReply &r) {
                       if (!r.ok) {
                           m_serialSub = 0;
                           m_earlySerialEvents.clear();
                           emit serialClosed(r.errorMessage);
                           return;
                       }
                       m_serialSub = r.result.toObject().value(u"subscription"_s).toInteger();
                       emit serialOpened(path, baud);
                       const auto early = std::exchange(m_earlySerialEvents, {});
                       for (const auto &[method, params] : early) {
                           if (m_serialSub > 0 && params.value(u"subscription"_s).toInteger() == m_serialSub)
                               handleSerialEvent(method, params);
                       }
                   });
}

void CoreConnection::handleSerialEvent(const QString &method, const QJsonObject &params)
{
    if (method == u"serial.data") {
        emit serialData(QByteArray::fromBase64(params.value(u"data"_s).toString().toLatin1()), params.value(u"shown"_s),
                        params.value(u"lines"_s).toArray());
        if (params.contains(u"explain"_s))
            emit outputExplained(u"serial"_s, params.value(u"explain"_s).toArray());
        if (params.contains(u"plot"_s))
            emit plotRows(params.value(u"plot"_s).toArray());
        return;
    }
    if (method == u"serial.bootLoop") {
        emit serialBootLoop(params);
        return;
    }
    if (method == u"serial.backtrace") {
        emit serialBacktrace(params.value(u"frames"_s).toArray());
        return;
    }
    if (method == u"serial.recordFailed") {
        emit serialRecordFailed(params.value(u"message"_s).toString(), params.value(u"bytes"_s).toInteger());
        return;
    }
    if (method == u"serial.garbled") {
        emit serialGarbled(params.value(u"baud"_s).toInt(), params.value(u"suggest"_s).toInt());
        return;
    }
    if (method == u"serial.expect") {
        const auto list = [&](const char *key) {
            QStringList out;
            for (const QJsonValue &v : params.value(QLatin1StringView(key)).toArray())
                out << v.toString();
            return out;
        };
        emit serialExpect(list("matched"), list("missing"), params.value(u"needs"_s).toString(),
                          params.value(u"seconds"_s).toInt(), params.value(u"timedOut"_s).toBool());
        return;
    }
    if (method == u"serial.lost") {
        emit serialLost(params.value(u"reason"_s).toString());
        return;
    }
    if (method == u"serial.reconnected") {
        emit serialReconnected();
        return;
    }
    m_serialSub = 0;
    emit serialClosed(params.value(u"reason"_s).toString());
}

void CoreConnection::closeSerial()
{
    if (m_serialSub <= 0)
        return;
    const qint64 sub = std::exchange(m_serialSub, 0);
    m_client->call(u"core.unsubscribe"_s, {{u"subscription"_s, sub}}, this, [](const RpcReply &) {});
    emit serialClosed(tr("Closed."));
}

void CoreConnection::writeSerial(const QByteArray &bytes)
{
    if (m_serialSub <= 0)
        return;
    m_client->call(u"serial.write"_s,
                   {{u"subscription"_s, m_serialSub}, {u"data"_s, QString::fromLatin1(bytes.toBase64())}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"serial.write"_s, r.errorMessage);
                   });
}

void CoreConnection::resetSerial()
{
    if (m_serialSub <= 0)
        return;
    m_client->call(u"serial.reset"_s, {{u"subscription"_s, m_serialSub}}, this, [this](const RpcReply &r) {
        if (!r.ok)
            emit requestFailed(u"serial.reset"_s, r.errorMessage);
    });
}

void CoreConnection::requestToolchainStatus()
{
    const QString method = u"toolchain.status"_s;
    m_client->call(method, {}, this, [this, method](const RpcReply &r) {
        if (r.ok)
            emit toolchainStatus(r.result.toObject().value(u"components"_s).toArray());
        else
            emit requestFailed(method, r.errorMessage);
    });
}

void CoreConnection::installComponent(const QString &component)
{
    if (m_installJob != 0)
        return; // one at a time
    m_installJob = -1;
    m_client->call(u"toolchain.install"_s, {{u"component"_s, component}}, this, [this](const RpcReply &r) {
        if (!r.ok) {
            m_installJob = 0;
            m_earlyInstallEvents.clear();
            emit toolchainInstallFinished(false, r.errorMessage);
            return;
        }
        m_installJob = r.result.toObject().value(u"job"_s).toInteger();
        const auto early = std::exchange(m_earlyInstallEvents, {});
        for (const auto &[method, params] : early) {
            if (m_installJob > 0 && params.value(u"subscription"_s).toInteger() == m_installJob)
                handleInstallEvent(method, params);
        }
    });
}

void CoreConnection::cancelInstall()
{
    if (m_installJob <= 0)
        return;
    const qint64 job = m_installJob;
    m_client->call(u"job.cancel"_s, {{u"job"_s, job}}, this, [this, job](const RpcReply &r) {
        // A cancelled job sends no toolchain.finished, so report it here.
        if (r.ok && m_installJob == job) {
            m_installJob = 0;
            emit toolchainInstallFinished(false, tr("Cancelled."));
        }
    });
}

void CoreConnection::handleInstallEvent(const QString &method, const QJsonObject &params)
{
    if (method == u"toolchain.progress") {
        emit toolchainProgress(params.value(u"line"_s).toString());
        return;
    }
    if (method == u"toolchain.download") {
        emit toolchainDownload(params.value(u"bytes"_s).toInteger(), params.value(u"total"_s).toInteger());
        return;
    }
    m_installJob = 0;
    emit toolchainInstallFinished(params.value(u"ok"_s).toBool(), params.value(u"error"_s).toString());
}

void CoreConnection::startBoardJob(const QString &operation, const QString &method, const QJsonObject &params,
                                   std::function<bool(const RpcReply &)> onRefused)
{
    if (m_boardJob != 0)
        return; // one board job at a time
    m_boardJob = -1;
    m_boardOperation = operation;
    const QString port = params.value(u"port"_s).toString();
    m_client->call(method, params, this, [this, operation, port, onRefused](const RpcReply &r) {
        if (!r.ok) {
            m_boardJob = 0;
            m_earlyBoardEvents.clear();
            if (!onRefused || !onRefused(r))
                emit boardFinished(operation, false, r.errorMessage, {});
            return;
        }
        m_boardJob = r.result.toObject().value(u"job"_s).toInteger();
        emit boardStarted(operation, port);
        const auto early = std::exchange(m_earlyBoardEvents, {});
        for (const auto &[m, p] : early) {
            if (m_boardJob > 0 && p.value(u"subscription"_s).toInteger() == m_boardJob)
                handleBoardEvent(m, p);
        }
    });
}

void CoreConnection::handleBoardEvent(const QString &method, const QJsonObject &params)
{
    if (method == u"board.progress") {
        emit boardProgress(params.value(u"stage"_s).toString(), params.value(u"written"_s).toInteger(),
                           params.value(u"total"_s).toInteger());
        return;
    }
    m_boardJob = 0;
    emit boardFinished(m_boardOperation, params.value(u"ok"_s).toBool(), params.value(u"error"_s).toString(),
                       params);
}

void CoreConnection::inspectBoard(const QString &port)
{
    startBoardJob(u"inspect"_s, u"board.inspect"_s, {{u"port"_s, port}});
}

void CoreConnection::eraseBoard(const QString &port)
{
    startBoardJob(u"erase"_s, u"board.erase"_s, {{u"port"_s, port}, {u"confirm"_s, true}});
}

void CoreConnection::flashBuild(const QString &port, const QString &project, const QString &board,
                                const QString &consent)
{
    QJsonObject params{{u"port"_s, port}, {u"project"_s, project}, {u"board"_s, board}};
    if (!consent.isEmpty())
        params[u"consent"_s] = consent;
    startBoardJob(u"flash"_s, u"board.flash"_s, params, [this, port, project, board](const RpcReply &r) {
        const QJsonObject data = r.errorData.toObject();
        if (r.errorCode != ConsentRequired || !data.contains(u"token"_s))
            return false;
        emit boardConsentRequired(port, project, board, data.value(u"token"_s).toString(),
                                  data.value(u"plan"_s).toObject());
        return true;
    });
}

void CoreConnection::startSim(const QString &project, const QString &board, bool debug)
{
    if (m_simSub != 0)
        return; // one simulator at a time
    m_simSub = -1;
    m_debugging = false;
    m_simPaused = false;
    m_simRecording = false;
    m_simProject = project;
    m_simBoard = board;
    m_client->call(u"sim.start"_s,
                   {{u"project"_s, project}, {u"board"_s, board}, {u"debug"_s, debug}, {u"icount"_s, m_simIcount}},
                   this,
                   [this, project, board](const RpcReply &r) {
                       if (!r.ok) {
                           m_simSub = 0;
                           m_earlySimEvents.clear();
                           emit simStopped(r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       m_simSub = o.value(u"subscription"_s).toInteger();
                       m_lastSimSub = m_simSub;
                       m_debugging = o.value(u"debug"_s).toBool();
                       m_simPins.clear();
                       m_simPartsActive = o.value(u"partsActive"_s).toBool();
                       m_simNetwork = o.value(u"network"_s).toObject();
                       for (const QJsonValue &g : o.value(u"pins"_s).toArray())
                           m_simPins << g.toInt();
                       m_traceSub = m_simPins.isEmpty() ? 0 : m_simSub;
                       emit simStarted(project, board, o.value(u"chip"_s).toString());
                       if (m_debugging)
                           emit debugStarted();
                       const auto early = std::exchange(m_earlySimEvents, {});
                       for (const auto &[m, p] : early) {
                           if (m_simSub > 0 && p.value(u"subscription"_s).toInteger() == m_simSub)
                               handleSimEvent(m, p);
                       }
                   });
}

void CoreConnection::handleSimEvent(const QString &method, const QJsonObject &params)
{
    if (method == u"sim.output") {
        emit simOutput(QByteArray::fromBase64(params.value(u"data"_s).toString().toLatin1()), params.value(u"shown"_s),
                       params.value(u"lines"_s).toArray());
        if (params.contains(u"explain"_s))
            emit outputExplained(u"sim"_s, params.value(u"explain"_s).toArray());
        if (params.contains(u"plot"_s))
            emit plotRows(params.value(u"plot"_s).toArray());
        return;
    }
    if (method == u"sim.expect") {
        const auto list = [&](const char *key) {
            QStringList out;
            for (const QJsonValue &v : params.value(QLatin1StringView(key)).toArray())
                out << v.toString();
            return out;
        };
        emit simExpect(list("matched"), list("missing"), params.value(u"needs"_s).toString(),
                       params.value(u"seconds"_s).toInt(), params.value(u"timedOut"_s).toBool());
        return;
    }
    if (method == u"sim.paused") {
        m_simPaused = params.value(u"paused"_s).toBool();
        emit simPaused(m_simPaused, params.contains(u"ns"_s) ? params.value(u"ns"_s).toInteger() : -1);
        return;
    }
    if (method == u"sim.recorded") {
        m_simRecording = false;
        emit simRecorded(params.value(u"path"_s).toString(), params.value(u"frames"_s).toInt(),
                         params.value(u"error"_s).toString());
        emit simRecordingChanged(false);
        return;
    }
    if (method == u"sim.bootLoop") {
        emit simBootLoop(params);
        return;
    }
    if (method == u"sim.backtrace") {
        emit simBacktrace(params.value(u"frames"_s).toArray());
        return;
    }
    if (method == u"sim.clock") {
        emit simClock(params.value(u"ns"_s).toInteger());
        return;
    }
    if (method == u"sim.wifiJoined") {
        emit simWifiJoined(params.value(u"ssid"_s).toString());
        return;
    }
    if (method == u"sim.partPicture") {
        const int w = params.value(u"width"_s).toInt();
        const int h = params.value(u"height"_s).toInt();
        QByteArray px = QByteArray::fromBase64(params.value(u"data"_s).toString().toLatin1());
        if (w <= 0 || h <= 0 || px.size() != qsizetype(w) * h)
            return;
        for (char &c : px)
            c = c ? char(0xff) : char(0);
        emit partPicture(params.value(u"id"_s).toString(),
                         QImage(reinterpret_cast<const uchar *>(px.constData()), w, h, w, QImage::Format_Grayscale8).copy());
        return;
    }
    if (method == u"sim.parts") {
        emit simPartsChanged(params.value(u"parts"_s).toArray());
        return;
    }
    if (method == u"sim.pins") {
        emit simPinsChanged(params.value(u"pins"_s).toArray());
        return;
    }
    if (method == u"sim.trace") {
        emit simTrace(params.value(u"edges"_s).toArray(), params.value(u"skipped"_s).toInteger());
        return;
    }
    if (method == u"sim.frame") {
        const int w = params.value(u"width"_s).toInt();
        const int h = params.value(u"height"_s).toInt();
        const QByteArray rgb = QByteArray::fromBase64(params.value(u"data"_s).toString().toLatin1());
        if (params.value(u"format"_s).toString() != u"rgb888" || w <= 0 || h <= 0
            || rgb.size() != qsizetype(w) * h * 3)
            return;
        // Copy: the QImage must not point into the temporary byte array.
        emit simFrame(QImage(reinterpret_cast<const uchar *>(rgb.constData()), w, h, w * 3, QImage::Format_RGB888).copy());
        return;
    }
    if (method == u"debug.stopped") {
        emit debugStopped(params.value(u"reason"_s).toString(), params.value(u"frame"_s).toObject(),
                          params.value(u"breakpoint"_s).toInt(), params.value(u"watch"_s).toObject(),
                          params.value(u"hits"_s).toInt());
        return;
    }
    if (method == u"debug.running") {
        emit debugRunning();
        return;
    }
    if (method == u"debug.exited") {
        m_debugging = false;
        emit debugExited();
        return;
    }
    if (method != u"sim.exited")
        return;
    m_simSub = 0;
    m_debugging = false;
    m_simPaused = false;
    if (std::exchange(m_simRecording, false))
        emit simRecordingChanged(false);
    m_simPins.clear();
    m_simPartsActive = false;
    m_simNetwork = {};
    emit simStopped(params.value(u"reason"_s).toString());
}

void CoreConnection::stopSim()
{
    if (m_simSub <= 0)
        return;
    const qint64 sub = std::exchange(m_simSub, 0);
    m_debugging = false;
    m_simPaused = false;
    const bool recording = std::exchange(m_simRecording, false);
    m_simPins.clear();
    m_simPartsActive = false;
    m_simNetwork = {};
    const auto unsubscribe = [this, sub] {
        m_client->call(u"core.unsubscribe"_s, {{u"subscription"_s, sub}}, this, [](const RpcReply &) {});
    };
    if (recording) {
        // Write the recording first: ending the subscription ends the session at once.
        emit simRecordingChanged(false);
        m_client->call(u"sim.record"_s, {{u"subscription"_s, sub}}, this, [this, unsubscribe](const RpcReply &r) {
            const QJsonObject o = r.result.toObject();
            emit simRecorded(o.value(u"path"_s).toString(), o.value(u"frames"_s).toInt(), r.ok ? QString() : r.errorMessage);
            unsubscribe();
        });
    } else {
        unsubscribe();
    }
    emit simStopped(tr("Stopped."));
}

void CoreConnection::saveTrace(const QString &path)
{
    if (m_traceSub <= 0)
        return;
    m_client->call(u"sim.traceSave"_s, {{u"subscription"_s, m_traceSub}, {u"path"_s, path}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"sim.traceSave"_s, r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       emit traceSaved(o.value(u"path"_s).toString(), o.value(u"edges"_s).toInteger(),
                                       o.value(u"dropped"_s).toInteger());
                   });
}

void CoreConnection::writeSim(const QByteArray &bytes)
{
    if (m_simSub <= 0)
        return;
    m_client->call(u"sim.write"_s, {{u"subscription"_s, m_simSub}, {u"data"_s, QString::fromLatin1(bytes.toBase64())}},
                   this, [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"sim.write"_s, r.errorMessage);
                   });
}

void CoreConnection::resetSim()
{
    if (m_simSub <= 0)
        return;
    m_client->call(u"sim.reset"_s, {{u"subscription"_s, m_simSub}}, this, [this](const RpcReply &r) {
        if (!r.ok)
            emit requestFailed(u"sim.reset"_s, r.errorMessage);
    });
}

void CoreConnection::debugBreak(const QString &location, const QString &condition, int ignore)
{
    if (!isDebugging())
        return;
    QJsonObject params{{u"subscription"_s, m_simSub}, {u"location"_s, location}};
    if (!condition.isEmpty())
        params.insert(u"condition"_s, condition);
    if (ignore > 0)
        params.insert(u"ignore"_s, ignore);
    m_client->call(u"debug.break"_s, params, this,
                   [this, location](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.break"_s, r.errorMessage);
                           return;
                       }
                       emit breakpointSet(r.result.toObject().value(u"breakpoint"_s).toObject(), location);
                   });
}

void CoreConnection::debugDelete(int breakpoint)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.delete"_s, {{u"subscription"_s, m_simSub}, {u"breakpoint"_s, breakpoint}}, this,
                   [this, breakpoint](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.delete"_s, r.errorMessage);
                           return;
                       }
                       emit breakpointDeleted(breakpoint);
                   });
}

void CoreConnection::debugControl(const QString &action)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.control"_s, {{u"subscription"_s, m_simSub}, {u"action"_s, action}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"debug.control"_s, r.errorMessage);
                   });
}

void CoreConnection::pauseSim(bool paused)
{
    if (m_simSub <= 0 || m_debugging)
        return;
    m_client->call(u"sim.pause"_s, {{u"subscription"_s, m_simSub}, {u"paused"_s, paused}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"sim.pause"_s, r.errorMessage);
                   });
}

void CoreConnection::debugRunTo(const QString &location)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.control"_s,
                   {{u"subscription"_s, m_simSub}, {u"action"_s, u"runTo"_s}, {u"location"_s, location}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"debug.control"_s, r.errorMessage);
                   });
}

void CoreConnection::debugEvaluate(const QString &expression)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.evaluate"_s, {{u"subscription"_s, m_simSub}, {u"expression"_s, expression}}, this,
                   [this, expression](const RpcReply &r) {
                       if (!r.ok) {
                           emit debugEvaluated(expression, {}, r.errorMessage);
                           return;
                       }
                       emit debugEvaluated(expression, r.result.toObject().value(u"value"_s).toString(), {});
                   });
}

void CoreConnection::debugInspect(int frame)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.inspect"_s, {{u"subscription"_s, m_simSub}, {u"frame"_s, frame}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.inspect"_s, r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       emit debugInspected(o.value(u"frames"_s).toArray(), o.value(u"registers"_s).toArray(),
                                           o.value(u"locals"_s).toArray(), o.value(u"frame"_s).toInt());
                   });
}

void CoreConnection::debugWatch(const QString &expression)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.watch"_s, {{u"subscription"_s, m_simSub}, {u"expression"_s, expression}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.watch"_s, r.errorMessage);
                           return;
                       }
                       const QJsonObject w = r.result.toObject().value(u"watchpoint"_s).toObject();
                       emit watchpointSet(w.value(u"number"_s).toInt(), w.value(u"expression"_s).toString());
                   });
}

void CoreConnection::debugChildren(const QString &expression)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.children"_s, {{u"subscription"_s, m_simSub}, {u"expression"_s, expression}}, this,
                   [this, expression](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.children"_s, r.errorMessage);
                           return;
                       }
                       emit childrenListed(expression, r.result.toObject().value(u"children"_s).toArray());
                   });
}

void CoreConnection::debugAssign(const QString &expression, const QString &value)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.assign"_s,
                   {{u"subscription"_s, m_simSub}, {u"expression"_s, expression}, {u"value"_s, value}}, this,
                   [this, expression](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.assign"_s, r.errorMessage);
                           return;
                       }
                       emit assigned(expression, r.result.toObject().value(u"value"_s).toString());
                   });
}

void CoreConnection::runSimFor(int ms)
{
    if (m_simSub <= 0 || m_debugging)
        return;
    m_client->call(u"sim.runFor"_s, {{u"subscription"_s, m_simSub}, {u"ms"_s, ms}}, this, [this](const RpcReply &r) {
        if (!r.ok)
            emit requestFailed(u"sim.runFor"_s, r.errorMessage);
    });
}

void CoreConnection::circuitExamples(const QString &kind)
{
    m_client->call(u"circuit.examples"_s, {{u"kind"_s, kind}}, this, [this, kind](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(u"circuit.examples"_s, r.errorMessage);
            return;
        }
        emit examplesListed(kind, r.result.toObject().value(u"projects"_s).toArray());
    });
}

void CoreConnection::debugMemory(const QString &address, int length)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.memory"_s, {{u"subscription"_s, m_simSub}, {u"address"_s, address}, {u"length"_s, length}},
                   this, [this, address](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.memory"_s, r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       emit memoryRead(address, o.value(u"address"_s).toString(),
                                       QByteArray::fromHex(o.value(u"bytes"_s).toString().toLatin1()));
                   });
}

void CoreConnection::debugIgnore(int number, int count)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.ignore"_s, {{u"subscription"_s, m_simSub}, {u"breakpoint"_s, number}, {u"count"_s, count}},
                   this, [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"debug.ignore"_s, r.errorMessage);
                   });
}

void CoreConnection::pulseSimPin(int gpio, bool level, int ms)
{
    if (m_simSub <= 0 || !m_simPins.contains(gpio))
        return;
    m_client->call(u"sim.pulsePin"_s,
                   {{u"subscription"_s, m_simSub}, {u"gpio"_s, gpio}, {u"level"_s, level}, {u"ms"_s, ms}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"sim.pulsePin"_s, r.errorMessage);
                   });
}

void CoreConnection::restartSim()
{
    if (!canRestartSim())
        return;
    const bool debug = m_debugging;
    const QString project = m_simProject, board = m_simBoard;
    stopSim();
    startSim(project, board, debug);
}

void CoreConnection::sendSerialFile(const QString &path, const QString &ending, int lineDelay)
{
    if (m_serialSub <= 0)
        return;
    m_client->call(u"serial.sendFile"_s,
                   {{u"subscription"_s, m_serialSub}, {u"path"_s, path}, {u"ending"_s, ending}, {u"lineDelay"_s, lineDelay}},
                   this, [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"serial.sendFile"_s, r.errorMessage);
                           return;
                       }
                       emit fileSent(u"serial"_s, r.result.toObject().value(u"lines"_s).toInt());
                   });
}

void CoreConnection::sendSimFile(const QString &path, const QString &ending, int lineDelay)
{
    if (m_simSub <= 0)
        return;
    m_client->call(u"sim.sendFile"_s,
                   {{u"subscription"_s, m_simSub}, {u"path"_s, path}, {u"ending"_s, ending}, {u"lineDelay"_s, lineDelay}},
                   this, [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"sim.sendFile"_s, r.errorMessage);
                           return;
                       }
                       emit fileSent(u"sim"_s, r.result.toObject().value(u"lines"_s).toInt());
                   });
}

void CoreConnection::recordSim(const QString &path)
{
    if (m_simSub <= 0)
        return;
    QJsonObject params{{u"subscription"_s, m_simSub}};
    if (!path.isEmpty())
        params.insert(u"path"_s, path);
    m_client->call(u"sim.record"_s, params, this, [this, path](const RpcReply &r) {
        if (!r.ok) {
            m_simRecording = false;
            emit requestFailed(u"sim.record"_s, r.errorMessage);
            emit simRecordingChanged(false);
            return;
        }
        const QJsonObject o = r.result.toObject();
        m_simRecording = !path.isEmpty();
        if (path.isEmpty())
            emit simRecorded(o.value(u"path"_s).toString(), o.value(u"frames"_s).toInt(), {});
        emit simRecordingChanged(m_simRecording);
    });
}

void CoreConnection::setSimPin(int gpio, bool level)
{
    if (m_simSub <= 0 || !m_simPins.contains(gpio))
        return;
    m_client->call(u"sim.setPin"_s, {{u"subscription"_s, m_simSub}, {u"gpio"_s, gpio}, {u"level"_s, level}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"sim.setPin"_s, r.errorMessage);
                   });
}

void CoreConnection::controlPart(const QString &id, const QString &control, const QJsonValue &value)
{
    if (!simPartsActive())
        return;
    m_client->call(u"sim.control"_s,
                   {{u"subscription"_s, m_simSub}, {u"part"_s, id}, {u"control"_s, control}, {u"value"_s, value}},
                   this, [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"sim.control"_s, r.errorMessage);
                   });
}

void CoreConnection::loadModels()
{
    m_client->call(u"circuit.models"_s, QJsonObject{}, this, [this](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(u"circuit.models"_s, r.errorMessage);
            return;
        }
        emit modelsLoaded(r.result.toObject().value(u"models"_s).toArray());
    });
}

void CoreConnection::loadCircuit(const QString &project, const QString &board)
{
    m_client->call(u"circuit.get"_s, {{u"project"_s, project}, {u"board"_s, board}}, this,
                   [this, project, board](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"circuit.get"_s, r.errorMessage);
                           return;
                       }
                       emit circuitLoaded(project, board, r.result.toObject().value(u"circuit"_s).toObject());
                   });
}

void CoreConnection::saveConsole(const QString &path, const QString &text)
{
    m_client->call(u"console.save"_s, {{u"path"_s, path}, {u"text"_s, text}}, this, [this, path](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(u"console.save"_s, r.errorMessage);
            return;
        }
        emit consoleSaved(path);
    });
}

void CoreConnection::savePlot(const QString &path, const QJsonArray &rows)
{
    m_client->call(u"plot.save"_s, {{u"path"_s, path}, {u"rows"_s, rows}}, this, [this, path](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(u"plot.save"_s, r.errorMessage);
            return;
        }
        emit plotSaved(path, r.result.toObject().value(u"rows"_s).toInteger());
    });
}

void CoreConnection::saveFrame(const QString &path, const QImage &image)
{
    // Rows of red, green and blue, as sim.frame carries them.
    const QImage rgb = image.convertToFormat(QImage::Format_RGB888);
    QByteArray data;
    data.reserve(rgb.width() * rgb.height() * 3);
    for (int y = 0; y < rgb.height(); ++y)
        data.append(reinterpret_cast<const char *>(rgb.constScanLine(y)), rgb.width() * 3);
    m_client->call(u"frame.save"_s,
                   {{u"path"_s, path}, {u"width"_s, rgb.width()}, {u"height"_s, rgb.height()},
                    {u"data"_s, QString::fromLatin1(data.toBase64())}},
                   this, [this, path](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"frame.save"_s, r.errorMessage);
                           return;
                       }
                       emit frameSaved(path);
                   });
}

void CoreConnection::exportSketch(const QString &project, const QString &board, const QString &dir, bool reuse)
{
    m_client->call(u"library.export"_s,
                   {{u"id"_s, project}, {u"board"_s, board}, {u"dir"_s, dir}, {u"reuse"_s, reuse}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"library.export"_s, r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       QStringList libs;
                       for (const QJsonValue &v : o.value(u"libraries"_s).toArray())
                           libs << v.toString();
                       emit sketchExported(o.value(u"path"_s).toString(), libs, o.value(u"existing"_s).toBool());
                   });
}

void CoreConnection::getCommands(const QString &project)
{
    m_client->call(u"commands.get"_s, {{u"project"_s, project}}, this, [this, project](const RpcReply &r) {
        if (!r.ok)
            return;
        QStringList out;
        for (const QJsonValue &v : r.result.toObject().value(u"commands"_s).toArray())
            out << v.toString();
        emit commandsListed(project, out);
    });
}

void CoreConnection::getBreakpoints(const QString &project)
{
    if (project.isEmpty())
        return;
    m_client->call(u"breakpoints.get"_s, {{u"project"_s, project}}, this, [this, project](const RpcReply &r) {
        if (!r.ok)
            return;
        QStringList out;
        for (const QJsonValue &v : r.result.toObject().value(u"breakpoints"_s).toArray())
            out << v.toString();
        emit breakpointsListed(project, out);
    });
}

void CoreConnection::setBreakpoints(const QString &project, const QStringList &locations)
{
    if (project.isEmpty())
        return;
    m_client->call(u"breakpoints.set"_s,
                   {{u"project"_s, project}, {u"breakpoints"_s, QJsonArray::fromStringList(locations)}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"breakpoints.set"_s, r.errorMessage);
                   });
}

void CoreConnection::getWatches(const QString &project)
{
    if (project.isEmpty())
        return;
    m_client->call(u"watches.get"_s, {{u"project"_s, project}}, this, [this, project](const RpcReply &r) {
        if (!r.ok)
            return;
        QStringList out;
        for (const QJsonValue &v : r.result.toObject().value(u"watches"_s).toArray())
            out << v.toString();
        emit watchesListed(project, out);
    });
}

void CoreConnection::setWatches(const QString &project, const QStringList &expressions)
{
    if (project.isEmpty())
        return;
    m_client->call(u"watches.set"_s,
                   {{u"project"_s, project}, {u"watches"_s, QJsonArray::fromStringList(expressions)}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"watches.set"_s, r.errorMessage);
                   });
}

void CoreConnection::decodeBacktrace(const QString &project, const QString &board, const QString &text)
{
    m_client->call(u"backtrace.decode"_s, {{u"project"_s, project}, {u"board"_s, board}, {u"text"_s, text}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"backtrace.decode"_s, r.errorMessage);
                           return;
                       }
                       emit backtraceDecoded(r.result.toObject().value(u"backtraces"_s).toArray());
                   });
}

void CoreConnection::recordSerial(const QString &path)
{
    if (m_serialSub <= 0)
        return;
    QJsonObject params{{u"subscription"_s, m_serialSub}};
    if (!path.isEmpty())
        params.insert(u"path"_s, path);
    m_client->call(u"serial.record"_s, params, this, [this](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(u"serial.record"_s, r.errorMessage);
            return;
        }
        const QJsonObject o = r.result.toObject();
        emit serialRecording(o.value(u"recording"_s).toBool(), o.value(u"path"_s).toString(),
                             o.value(u"bytes"_s).toInteger());
    });
}

void CoreConnection::setCommands(const QString &project, const QStringList &commands)
{
    m_client->call(u"commands.set"_s, {{u"project"_s, project}, {u"commands"_s, QJsonArray::fromStringList(commands)}},
                   this, [this, project](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"commands.set"_s, r.errorMessage);
                           return;
                       }
                       QStringList out;
                       for (const QJsonValue &v : r.result.toObject().value(u"commands"_s).toArray())
                           out << v.toString();
                       emit commandsListed(project, out);
                   });
}

void CoreConnection::requestBuildStatus(const QString &project, const QString &board)
{
    m_client->call(u"build.status"_s, {{u"project"_s, project}, {u"board"_s, board}}, this,
                   [this, project, board](const RpcReply &r) {
                       if (r.ok)
                           emit buildStatusReady(project, board, r.result.toObject());
                   });
}

void CoreConnection::suggestCircuit(const QString &project, const QString &board)
{
    m_client->call(u"circuit.suggest"_s, {{u"project"_s, project}, {u"board"_s, board}}, this,
                   [this, project, board](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"circuit.suggest"_s, r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       QStringList unmatched;
                       for (const QJsonValue &v : o.value(u"unmatched"_s).toArray())
                           unmatched << v.toString();
                       emit circuitSuggested(project, board, o.value(u"circuit"_s).toObject(), unmatched);
                   });
}

void CoreConnection::saveCircuit(const QString &project, const QString &board, const QJsonObject &circuit)
{
    m_client->call(u"circuit.set"_s, {{u"project"_s, project}, {u"board"_s, board}, {u"circuit"_s, circuit}}, this,
                   [this, project, board, circuit](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"circuit.set"_s, r.errorMessage);
                           return;
                       }
                       emit circuitSaved(project, board);
                       emit circuitLoaded(project, board, circuit);
                   });
}
