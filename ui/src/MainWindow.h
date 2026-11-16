#pragma once

#include <QJsonObject>
#include <QMainWindow>

class CoreConnection;
class BuildPanel;
class DevicesPanel;
class LibraryPanel;
class ProjectView;
class QSplitter;
class SerialMonitor;
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

    // Ask the user whether to build a downloaded project. Returns true to build.
    // Virtual so tests can answer without a modal dialog.
    virtual bool confirmBuild(const QString &project, const QString &board, const QJsonObject &plan);

protected:
    void closeEvent(QCloseEvent *event) override;

private:
    void buildMenus();
    void showConnectionState();
    void startCore();
    void restoreLayout();
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
    QLabel *m_connection;
    QPushButton *m_startCore;
};
