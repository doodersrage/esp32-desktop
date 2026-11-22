// The simulation's pins: one row per GPIO the project uses (the core decides which,
// from the library), with what the firmware does with it, and a switch to drive it as
// an input. The first step towards virtual parts.

#pragma once

#include <QJsonArray>
#include <QHash>
#include <QSet>
#include <QWidget>

class CoreConnection;
class QLabel;
class QTreeWidget;
class QTreeWidgetItem;

class PinsPanel : public QWidget {
    Q_OBJECT

public:
    explicit PinsPanel(CoreConnection *core, QWidget *parent = nullptr);

    enum Column { GpioColumn, DirectionColumn, LevelColumn, RateColumn, DriveColumn, ColumnCount };
    // sim.pins' rate as shown: "1.00 Hz · 50% high", or empty.
    static QString rateText(const QJsonValue &rate);

    QString statusText() const;
    QTreeWidget *tree() const { return m_tree; }
    // The row for `gpio`, or null.
    QTreeWidgetItem *row(int gpio) const;

    // What each pin is wired to (library.get's pinLabels), shown beside its number.
    void setLabels(const QHash<int, QString> &labels);
    // Drive a pin the other way from its switch for `ms`, then back: a button pressed
    // and let go (sim.pulsePin; the core times it). Also in a row's right-click menu.
    bool pulse(int gpio, int ms);
    // Light a pin's row for a moment (its level changed).
    void flash(int gpio);
    bool isLit(int gpio) const { return m_lit.contains(gpio); }
    static constexpr int FlashMs = 400;

signals:
    // A double-click or Enter on a pin's row: show it in the trace.
    void pinActivated(int gpio);

private:
    void started();
    void update(const QJsonArray &pins);
    void clearRows(const QString &status);

    CoreConnection *m_core;
    QLabel *m_status;
    QTreeWidget *m_tree;
    QHash<int, QString> m_labels;
    QString m_board; // the running simulation's
    QSet<int> m_lit; // rows lit by a change
};
