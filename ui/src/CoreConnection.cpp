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
        m_devicesSubscription = 0; // the core drops subscriptions and jobs with the connection
        m_earlyLibraryEvents.clear();
        m_earlyBuildEvents.clear();
        m_earlySerialEvents.clear();
        m_earlyInstallEvents.clear();
        m_earlyBoardEvents.clear();
        m_earlySimEvents.clear();
        if (m_simSub != 0) {
            m_simSub = 0;
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
        {u"protocol"_s, u"0.15.0"_s},
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
    if (sub == 0)
        return;
    if (method == u"device.changed" && sub == m_devicesSubscription) {
        emit devicesListed(params.value(u"ports"_s).toArray());
    } else if (method == u"build.log" || method == u"build.finished") {
        if (sub == m_buildJob)
            handleBuildEvent(method, params);
        else if (m_buildJob == -1)
            m_earlyBuildEvents.append({method, params}); // our job id is not known yet
    } else if (method == u"sim.output" || method == u"sim.frame" || method == u"sim.pins" || method == u"sim.parts"
               || method == u"sim.exited"
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
    } else if (method == u"toolchain.progress" || method == u"toolchain.finished") {
        if (sub == m_installJob)
            handleInstallEvent(method, params);
        else if (m_installJob == -1)
            m_earlyInstallEvents.append({method, params}); // our job id is not known yet
    } else if (method == u"serial.data" || method == u"serial.closed") {
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
        emit buildLog(params.value(u"stream"_s).toString(), params.value(u"line"_s).toString());
        return;
    }
    m_buildJob = 0;
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

void CoreConnection::openSerial(const QString &path, int baud)
{
    if (m_serialSub != 0)
        return; // one port at a time
    m_serialSub = -1;
    m_client->call(u"serial.open"_s, {{u"path"_s, path}, {u"baud"_s, baud}}, this,
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
        emit serialData(QByteArray::fromBase64(params.value(u"data"_s).toString().toLatin1()));
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

void CoreConnection::handleInstallEvent(const QString &method, const QJsonObject &params)
{
    if (method == u"toolchain.progress") {
        emit toolchainProgress(params.value(u"line"_s).toString());
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
    m_client->call(u"sim.start"_s, {{u"project"_s, project}, {u"board"_s, board}, {u"debug"_s, debug}}, this,
                   [this, project, board](const RpcReply &r) {
                       if (!r.ok) {
                           m_simSub = 0;
                           m_earlySimEvents.clear();
                           emit simStopped(r.errorMessage);
                           return;
                       }
                       const QJsonObject o = r.result.toObject();
                       m_simSub = o.value(u"subscription"_s).toInteger();
                       m_debugging = o.value(u"debug"_s).toBool();
                       m_simPins.clear();
                       m_simPartsActive = o.value(u"partsActive"_s).toBool();
                       for (const QJsonValue &g : o.value(u"pins"_s).toArray())
                           m_simPins << g.toInt();
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
        emit simOutput(QByteArray::fromBase64(params.value(u"data"_s).toString().toLatin1()));
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
                          params.value(u"breakpoint"_s).toInt());
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
    m_simPins.clear();
    m_simPartsActive = false;
    emit simStopped(params.value(u"reason"_s).toString());
}

void CoreConnection::stopSim()
{
    if (m_simSub <= 0)
        return;
    const qint64 sub = std::exchange(m_simSub, 0);
    m_debugging = false;
    m_simPins.clear();
    m_simPartsActive = false;
    m_client->call(u"core.unsubscribe"_s, {{u"subscription"_s, sub}}, this, [](const RpcReply &) {});
    emit simStopped(tr("Stopped."));
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

void CoreConnection::debugBreak(const QString &location)
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.break"_s, {{u"subscription"_s, m_simSub}, {u"location"_s, location}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"debug.break"_s, r.errorMessage);
                           return;
                       }
                       emit breakpointSet(r.result.toObject().value(u"breakpoint"_s).toObject());
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

void CoreConnection::debugInspect()
{
    if (!isDebugging())
        return;
    m_client->call(u"debug.inspect"_s, {{u"subscription"_s, m_simSub}}, this, [this](const RpcReply &r) {
        if (!r.ok) {
            emit requestFailed(u"debug.inspect"_s, r.errorMessage);
            return;
        }
        const QJsonObject o = r.result.toObject();
        emit debugInspected(o.value(u"frames"_s).toArray(), o.value(u"registers"_s).toArray());
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

void CoreConnection::pressPart(const QString &id, bool pressed)
{
    if (!simPartsActive())
        return;
    m_client->call(u"sim.press"_s, {{u"subscription"_s, m_simSub}, {u"part"_s, id}, {u"pressed"_s, pressed}}, this,
                   [this](const RpcReply &r) {
                       if (!r.ok)
                           emit requestFailed(u"sim.press"_s, r.errorMessage);
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

void CoreConnection::saveCircuit(const QString &project, const QString &board, const QJsonObject &circuit)
{
    m_client->call(u"circuit.set"_s, {{u"project"_s, project}, {u"board"_s, board}, {u"circuit"_s, circuit}}, this,
                   [this, project, board, circuit](const RpcReply &r) {
                       if (!r.ok) {
                           emit requestFailed(u"circuit.set"_s, r.errorMessage);
                           return;
                       }
                       emit circuitLoaded(project, board, circuit);
                   });
}
