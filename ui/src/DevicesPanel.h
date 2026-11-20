// Lists serial ports the core found (device.list). Renders only; the core decides
// what each port is.

#pragma once

#include <QJsonValue>
#include <QJsonArray>
#include <QWidget>

class CoreConnection;
class QLabel;
class QToolButton;
class QTreeWidget;

class DevicesPanel : public QWidget {
    Q_OBJECT

public:
    enum Column { PortColumn, ChipsColumn, BridgeColumn, UsbIdColumn, ColumnCount };

    explicit DevicesPanel(CoreConnection *core, QWidget *parent = nullptr);

    // A device.list bridge id ("cp210x") as people know it ("CP210x USB to UART").
    static QString bridgeName(const QJsonValue &bridge);
    // What to do about a port the user may not open (device.list's access "denied").
    static QString accessHint();

    // Render a device.list result. Public so tests can feed fixed data.
    void showPorts(const QJsonArray &ports);
    QTreeWidget *tree() const { return m_tree; }
    QString statusText() const;

public slots:
    void refresh();

signals:
    // The user double-clicked a port (to open it in the serial monitor).
    void portActivated(const QString &path);
    // From the port's context menu.
    void inspectRequested(const QString &path);
    void eraseRequested(const QString &path);

private:
    void showMessage(const QString &text);
    void updateEnabled();

    CoreConnection *m_core;
    QTreeWidget *m_tree;
    QLabel *m_status;
    QToolButton *m_refresh;
};
