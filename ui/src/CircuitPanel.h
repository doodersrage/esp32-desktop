// The virtual parts on the current project's board: a canvas with the board's pins on
// the left, wired to its parts on the right. The kinds of part come from the core
// (circuit.models); parts are added and removed here and saved through the core
// (circuit.set), which checks them. While a simulation runs, parts show the state the
// core reports (sim.parts), and their controls work by mouse (sim.control): hold a
// button, click a switch.

#pragma once

#include <QHash>
#include <QImage>
#include <QJsonArray>
#include <QJsonObject>
#include <QPointF>
#include <QWidget>

class CoreConnection;
class QGraphicsScene;
class QGraphicsView;
class QLabel;
class QPushButton;
class QToolButton;
class QDoubleSpinBox;
class QGroupBox;
class PartItem;

class CircuitPanel : public QWidget {
    Q_OBJECT

public:
    explicit CircuitPanel(CoreConnection *core, QWidget *parent = nullptr);

    // Show (and load the circuit of) a project's board.
    void showBoard(const QString &project, const QString &board, const QJsonArray &pins);
    // Add a part of model `kind` and save: `pins` maps the model's pin names to GPIOs.
    void addPart(const QString &kind, const QJsonObject &pins, const QJsonObject &props, const QString &label,
                 const QJsonObject &i2c = {});
    // The same, for a one-pin model with an activeLow property.
    void addPart(const QString &kind, int pin, bool activeLow, const QString &label);
    void removePart(const QString &id);

    // For tests.
    QString statusText() const;
    QJsonArray parts() const { return m_circuit.value(QStringLiteral("parts")).toArray(); }
    QStringList modelIds() const { return m_modelOrder; }
    bool isOn(const QString &id) const;
    QJsonObject partState(const QString &id) const;
    // A display part's latest picture (null until the simulation draws).
    QImage partPicture(const QString &id) const;
    QString partToolTip(const QString &id) const;
    QGraphicsView *view() const { return m_view; }
    PartItem *item(const QString &id) const { return m_items.value(id); }
    // The middle of a part on the canvas, in scene coordinates.
    QPointF partCentre(const QString &id) const;
    QToolButton *addPartButton() const { return m_add; }
    // The spin box for a selected part's number control, or null.
    QDoubleSpinBox *controlBox(const QString &name) const;
    // Select a part on the canvas, as a click does.
    void selectPart(const QString &id, bool selected);

private:
    void circuitLoaded(const QString &project, const QString &board, const QJsonObject &circuit);
    void modelsLoaded(const QJsonArray &models);
    void rebuildScene();
    void resetStates();
    void addDialog(const QString &kind);
    QString freeId(const QString &kind) const;
    void updateStatus();
    void rebuildControls();

    CoreConnection *m_core;
    QLabel *m_status;
    QToolButton *m_add;
    QPushButton *m_remove;
    QGraphicsScene *m_scene;
    QGraphicsView *m_view;
    QGroupBox *m_controls;
    QString m_project;
    QString m_board;
    QList<int> m_projectPins;
    QJsonObject m_circuit;
    QHash<QString, QJsonObject> m_models;
    QStringList m_modelOrder;
    QHash<QString, PartItem *> m_items;
};
