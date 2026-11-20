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
class PlotPanel;
class QDockWidget;
class QLabel;
class QPushButton;
class QAction;
class QToolBar;
class QToolButton;
class QProgressBar;
class MessageBar;

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
    QToolBar *toolBar() const { return m_toolBar; }
    QString nextStepText() const;
    MessageBar *messageBar() const { return m_messages; }
    // The toolbar's Board menu: serial monitor, inspect and erase for the chosen port.
    QToolButton *boardMenuButton() const { return m_boardMenu; }
    CircuitPanel *circuit() const { return m_circuit; }
    PlotPanel *plot() const { return m_plot; }
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
    void buildToolBar();
    // The toolbar's project, Stop and next-step hint, from what the window shows now.
    void updateToolBar();
    // The first-start layout: library, project, simulator and circuit; the rest open
    // when they have something to show. View → Reset layout returns to it.
    void resetLayout();
    void applyDefaultSizes();
    QStringList m_pendingLinks;
    void showConnectionState();
    void startCore();
    void restoreLayout();
    void freePort(const QString &port);
    void openSerialPort(const QString &path);
    void inspectPort(const QString &port);
    void erasePort(const QString &port);
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
    QDockWidget *m_plotDock = nullptr;
    PlotPanel *m_plot = nullptr;
    TracePanel *m_trace = nullptr;
    QString m_autoStartError;
    SimulatorPanel *m_sim = nullptr;
    BoardInfoDialog *m_boardInfo = nullptr;
    // A serial monitor closed to free the port for flashing, reopened afterwards.
    QString m_reopenSerial;
    QString m_lastBoardPort;
    // The project the last flash wrote, for the serial monitor's baud rate after it.
    QString m_flashedProject;
    // Try Blink was pressed: run it once it is shown and can be.
    bool m_tryPending = false;
    QToolBar *m_toolBar = nullptr;
    QLabel *m_current = nullptr;
    QLabel *m_nextStep = nullptr;
    QAction *m_stopSim = nullptr;
    QToolButton *m_boardMenu = nullptr;
    QProgressBar *m_busy = nullptr;
    QAction *m_busyAction = nullptr;
    MessageBar *m_messages = nullptr;
    // arduino-cli or the ESP32 core is not installed (toolchain.status).
    bool m_buildToolsMissing = false;
    // The last project was reopened (or a link chose one) at the first connection.
    bool m_reopened = false;
    QByteArray m_defaultState;
    QLabel *m_connection;
    QPushButton *m_startCore;
};
