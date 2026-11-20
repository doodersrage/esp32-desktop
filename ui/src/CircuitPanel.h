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
    // Keep a part where it was dragged to on the canvas (circuit 1.3's `at`).
    void movePart(const QString &id, QPointF to);
    // Back to the parts as they were before the last change (add, remove, move).
    void undo();
    QPushButton *undoButton() const { return m_undoButton; }

    // For tests.
    QString statusText() const;
    QJsonArray parts() const { return m_circuit.value(QStringLiteral("parts")).toArray(); }
    QStringList modelIds() const { return m_modelOrder; }
    bool isOn(const QString &id) const;
    QJsonObject partState(const QString &id) const;
    // A display part's latest picture (null until the simulation draws).
    QImage partPicture(const QString &id) const;
    QString partToolTip(const QString &id) const;
    // A strip part's number of LEDs as drawn (0 for other parts).
    int partStripLength(const QString &id) const;
    QGraphicsView *view() const { return m_view; }
    PartItem *item(const QString &id) const { return m_items.value(id); }
    // The middle of a part on the canvas, in scene coordinates.
    QPointF partCentre(const QString &id) const;
    QToolButton *addPartButton() const { return m_add; }
    // Shown when the board has no parts and the project's wiring names some the
    // simulator has (circuit.suggest): places them all.
    QPushButton *addSuggestedButton() const { return m_addSuggested; }
    // The spin box for a selected part's number control, or null.
    QDoubleSpinBox *controlBox(const QString &name) const;
    // Select a part on the canvas, as a click does.
    void selectPart(const QString &id, bool selected);

private:
    void circuitLoaded(const QString &project, const QString &board, const QJsonObject &circuit);
    void circuitSuggested(const QString &project, const QString &board, const QJsonObject &circuit,
                          const QStringList &unmatched);
    void modelsLoaded(const QJsonArray &models);
    void rebuildScene();
    void resetStates();
    void addDialog(const QString &kind);
    QString freeId(const QString &kind) const;
    void updateStatus();
    void rebuildControls();
    void saveControl(const QString &id, const QString &name, double value);
    // Save a change the user made, remembering what it replaces for undo().
    void edit(const QJsonObject &circuit);

    CoreConnection *m_core;
    QLabel *m_status;
    QToolButton *m_add;
    QPushButton *m_remove;
    QPushButton *m_addSuggested;
    QPushButton *m_undoButton;
    // The board's circuits before each change, newest last.
    QList<QJsonObject> m_undo;
    // What the project's wiring gives (circuit.suggest) for the board shown.
    QJsonObject m_suggested;
    QStringList m_unmatched;
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
