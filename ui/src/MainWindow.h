#pragma once

#include <QJsonObject>
#include <QMainWindow>

class CoreConnection;
class BoardInfoDialog;
class BuildPanel;
class DevicesPanel;
class LibraryPanel;
class ProjectView;
class QSplitter;
class SerialMonitor;
class SetupDialog;
class SimulatorPanel;
class DebugPanel;
class DisplayPanel;
class PinsPanel;
class TracePanel;
class CircuitPanel;
class QDockWidget;
class QLabel;
class QPushButton;

class MainWindow : public QMainWindow {
    Q_OBJECT

public:
    explicit MainWindow(CoreConnection *core, QWidget *parent = nullptr);

    DevicesPanel *devicesPanel() const { return m_devices; }
    LibraryPanel *libraryPanel() const { return m_library; }
    ProjectView *projectView() const { return m_project; }
    QString connectionText() const;
    BuildPanel *buildPanel() const { return m_buildPanel; }
    SerialMonitor *serialMonitor() const { return m_serial; }
    SetupDialog *setupDialog() const { return m_setup; }
    SimulatorPanel *simulator() const { return m_sim; }
    DebugPanel *debugger() const { return m_debugger; }
    DisplayPanel *display() const { return m_display; }
    PinsPanel *pins() const { return m_pins; }
    TracePanel *trace() const { return m_trace; }
    CircuitPanel *circuit() const { return m_circuit; }
    void showSetup();
    // Open an esp32bench:// link: select the project (and board) it names. It is
    // not built or flashed; that still asks first. Waits for the core if needed.
    void openLink(const QString &url);

    // Ask the user whether to build a downloaded project. Returns true to build.
    // Virtual so tests can answer without a modal dialog.
    virtual bool confirmBuild(const QString &project, const QString &board, const QJsonObject &plan);
    // Same for flashing a downloaded project, and for erasing a board.
    virtual bool confirmFlash(const QString &port, const QJsonObject &plan);
    virtual bool confirmErase(const QString &port);
    BoardInfoDialog *boardInfo() const { return m_boardInfo; }

protected:
    void closeEvent(QCloseEvent *event) override;

private:
    void buildMenus();
    QStringList m_pendingLinks;
    void showConnectionState();
    void startCore();
    void restoreLayout();
    void freePort(const QString &port);
    void saveLayout();

    CoreConnection *m_core;
    DevicesPanel *m_devices;
    LibraryPanel *m_library = nullptr;
    ProjectView *m_project = nullptr;
    QSplitter *m_split = nullptr;
    QDockWidget *m_devicesDock;
    QDockWidget *m_buildDock = nullptr;
    BuildPanel *m_buildPanel = nullptr;
    QDockWidget *m_serialDock = nullptr;
    SerialMonitor *m_serial = nullptr;
    SetupDialog *m_setup = nullptr;
    QDockWidget *m_simDock = nullptr;
    QDockWidget *m_debugDock = nullptr;
    DebugPanel *m_debugger = nullptr;
    QDockWidget *m_displayDock = nullptr;
    DisplayPanel *m_display = nullptr;
    QDockWidget *m_pinsDock = nullptr;
    PinsPanel *m_pins = nullptr;
    QDockWidget *m_circuitDock = nullptr;
    CircuitPanel *m_circuit = nullptr;
    QDockWidget *m_traceDock = nullptr;
    TracePanel *m_trace = nullptr;
    QString m_autoStartError;
    SimulatorPanel *m_sim = nullptr;
    BoardInfoDialog *m_boardInfo = nullptr;
    // A serial monitor closed to free the port for flashing, reopened afterwards.
    QString m_reopenSerial;
    QString m_lastBoardPort;
    QLabel *m_connection;
    QPushButton *m_startCore;
};
