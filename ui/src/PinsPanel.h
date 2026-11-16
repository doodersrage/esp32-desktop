// The simulation's pins: one row per GPIO the project uses (the core decides which,
// from the library), with what the firmware does with it, and a switch to drive it as
// an input. The first step towards virtual parts.

#pragma once

#include <QJsonArray>
#include <QWidget>

class CoreConnection;
class QLabel;
class QTreeWidget;
class QTreeWidgetItem;

class PinsPanel : public QWidget {
    Q_OBJECT

public:
    explicit PinsPanel(CoreConnection *core, QWidget *parent = nullptr);

    enum Column { GpioColumn, DirectionColumn, LevelColumn, DriveColumn, ColumnCount };

    QString statusText() const;
    QTreeWidget *tree() const { return m_tree; }
    // The row for `gpio`, or null.
    QTreeWidgetItem *row(int gpio) const;

private:
    void started();
    void update(const QJsonArray &pins);
    void clearRows(const QString &status);

    CoreConnection *m_core;
    QLabel *m_status;
    QTreeWidget *m_tree;
};
