// The virtual parts on the current project's board: a canvas with the board's pins on
// the left, wired to LEDs and buttons on the right. Parts are added and removed here
// and saved through the core (circuit.set), which checks them. While a simulation
// runs, LEDs light as the core reports (sim.parts) and buttons are pressed by holding
// the mouse on them (sim.press).

#pragma once

#include <QHash>
#include <QJsonArray>
#include <QJsonObject>
#include <QPointF>
#include <QWidget>

class CoreConnection;
class QGraphicsScene;
class QGraphicsView;
class QLabel;
class QPushButton;
class PartItem;

class CircuitPanel : public QWidget {
    Q_OBJECT

public:
    explicit CircuitPanel(CoreConnection *core, QWidget *parent = nullptr);

    // Show (and load the circuit of) a project's board.
    void showBoard(const QString &project, const QString &board, const QJsonArray &pins);
    // Add a part and save. kind is "led" or "button".
    void addPart(const QString &kind, int pin, bool activeLow, const QString &label);
    void removePart(const QString &id);

    // For tests.
    QString statusText() const;
    QJsonArray parts() const { return m_circuit.value(QStringLiteral("parts")).toArray(); }
    bool isOn(const QString &id) const;
    QGraphicsView *view() const { return m_view; }
    PartItem *item(const QString &id) const { return m_items.value(id); }
    // The middle of a part on the canvas, in scene coordinates.
    QPointF partCentre(const QString &id) const;
    QPushButton *addLedButton() const { return m_addLed; }

private:
    void circuitLoaded(const QString &project, const QString &board, const QJsonObject &circuit);
    void rebuildScene();
    void setAllOff();
    void addDialog(const QString &kind);
    QString freeId(const QString &kind) const;
    void updateStatus();

    CoreConnection *m_core;
    QLabel *m_status;
    QPushButton *m_addLed;
    QPushButton *m_addButton;
    QPushButton *m_remove;
    QGraphicsScene *m_scene;
    QGraphicsView *m_view;
    QString m_project;
    QString m_board;
    QList<int> m_projectPins;
    QJsonObject m_circuit;
    QHash<QString, PartItem *> m_items;
};
