#pragma once

#include <QMainWindow>

class CoreConnection;
class DevicesPanel;
class LibraryPanel;
class ProjectView;
class QSplitter;
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
    QLabel *m_connection;
    QPushButton *m_startCore;
};
