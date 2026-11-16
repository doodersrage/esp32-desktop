#include "CircuitPanel.h"

#include "CoreConnection.h"

#include <QCheckBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QGraphicsItem>
#include <QGraphicsScene>
#include <QGraphicsSceneMouseEvent>
#include <QGraphicsView>
#include <QHBoxLayout>
#include <QIcon>
#include <QKeyEvent>
#include <QLabel>
#include <QLineEdit>
#include <QPainter>
#include <QPushButton>
#include <QSpinBox>
#include <QStyleOptionGraphicsItem>
#include <QVBoxLayout>

#include <algorithm>
#include <functional>

using namespace Qt::StringLiterals;

namespace {
constexpr qreal RowHeight = 44;
constexpr qreal BoardWidth = 110;
constexpr qreal PartX = 170;
constexpr qreal PartSize = 30;
}

// One part on the canvas. Colours are the canvas's own (rule: hard-coded colours only
// on the canvas): an LED is red when lit. Text follows the system palette.
class PartItem : public QGraphicsItem {
public:
    PartItem(const QString &id, const QString &kind, const QString &label, std::function<void(bool)> press)
        : m_id(id)
        , m_kind(kind)
        , m_label(label)
        , m_press(std::move(press))
    {
        setFlag(ItemIsSelectable);
        setAcceptedMouseButtons(Qt::LeftButton);
    }

    QString id() const { return m_id; }
    bool isOn() const { return m_on; }
    void setOn(bool on)
    {
        if (on != m_on) {
            m_on = on;
            update();
        }
    }

    QRectF boundingRect() const override { return {-2, -2, PartSize + 160, PartSize + 4}; }

    void paint(QPainter *p, const QStyleOptionGraphicsItem *option, QWidget *widget) override
    {
        p->setRenderHint(QPainter::Antialiasing);
        const QPalette pal = widget ? widget->palette() : QPalette();
        const QRectF body(0, 0, PartSize, PartSize);
        if (m_kind == u"led") {
            p->setPen(QPen(pal.color(QPalette::Text), 1.5));
            p->setBrush(m_on ? QColor(0xe5, 0x39, 0x35) : QColor(0x5a, 0x22, 0x22));
            p->drawEllipse(body);
            if (m_on) { // a glow, so lit reads at a glance
                p->setPen(Qt::NoPen);
                p->setBrush(QColor(0xff, 0x8a, 0x80, 120));
                p->drawEllipse(body.adjusted(6, 6, -6, -6));
            }
        } else {
            p->setPen(QPen(pal.color(QPalette::Text), 1.5));
            p->setBrush(pal.color(m_on ? QPalette::Highlight : QPalette::Button));
            p->drawRoundedRect(body, 6, 6);
            p->setBrush(pal.color(m_on ? QPalette::HighlightedText : QPalette::Mid));
            p->drawEllipse(body.adjusted(8, 8, -8, -8));
        }
        if (option->state & QStyle::State_Selected) {
            p->setPen(QPen(pal.color(QPalette::Highlight), 1, Qt::DashLine));
            p->setBrush(Qt::NoBrush);
            p->drawRect(body.adjusted(-2, -2, 2, 2));
        }
        p->setPen(pal.color(QPalette::Text));
        p->drawText(QRectF(PartSize + 8, 0, 150, PartSize), Qt::AlignVCenter | Qt::AlignLeft, m_label);
    }

protected:
    void mousePressEvent(QGraphicsSceneMouseEvent *event) override
    {
        QGraphicsItem::mousePressEvent(event);
        if (m_kind == u"button")
            m_press(true);
    }
    void mouseReleaseEvent(QGraphicsSceneMouseEvent *event) override
    {
        QGraphicsItem::mouseReleaseEvent(event);
        if (m_kind == u"button")
            m_press(false);
    }

private:
    QString m_id;
    QString m_kind;
    QString m_label;
    std::function<void(bool)> m_press;
    bool m_on = false;
};

CircuitPanel::CircuitPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(this))
    , m_addLed(new QPushButton(QIcon::fromTheme(u"list-add"_s), tr("Add LED"), this))
    , m_addButton(new QPushButton(QIcon::fromTheme(u"list-add"_s), tr("Add button"), this))
    , m_remove(new QPushButton(QIcon::fromTheme(u"list-remove"_s), tr("Remove"), this))
    , m_scene(new QGraphicsScene(this))
    , m_view(new QGraphicsView(m_scene, this))
{
    m_status->setWordWrap(true);
    m_view->setRenderHint(QPainter::Antialiasing);
    m_view->setAlignment(Qt::AlignLeft | Qt::AlignTop);
    m_addLed->setToolTip(tr("Add an LED on one of the board's pins"));
    m_addButton->setToolTip(tr("Add a push button; hold the mouse on it to press it while the simulator runs"));
    m_remove->setToolTip(tr("Remove the selected part"));
    connect(m_addLed, &QPushButton::clicked, this, [this] { addDialog(u"led"_s); });
    connect(m_addButton, &QPushButton::clicked, this, [this] { addDialog(u"button"_s); });
    connect(m_remove, &QPushButton::clicked, this, [this] {
        for (QGraphicsItem *i : m_scene->selectedItems()) {
            if (auto *part = dynamic_cast<PartItem *>(i)) {
                removePart(part->id());
                return;
            }
        }
    });
    // m_remove as the context: the scene emits selectionChanged while it is destroyed,
    // after the button has gone.
    connect(m_scene, &QGraphicsScene::selectionChanged, m_remove,
            [this] { m_remove->setEnabled(!m_scene->selectedItems().isEmpty()); });

    auto *tools = new QHBoxLayout;
    tools->addWidget(m_addLed);
    tools->addWidget(m_addButton);
    tools->addWidget(m_remove);
    tools->addStretch(1);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_status);
    layout->addLayout(tools);
    layout->addWidget(m_view, 1);

    connect(core, &CoreConnection::circuitLoaded, this, &CircuitPanel::circuitLoaded);
    connect(core, &CoreConnection::simPartsChanged, this, [this](const QJsonArray &parts) {
        for (const QJsonValue &v : parts) {
            const QJsonObject o = v.toObject();
            if (PartItem *i = m_items.value(o.value(u"id"_s).toString()))
                i->setOn(o.value(u"on"_s).toBool());
        }
    });
    connect(core, &CoreConnection::simStarted, this, [this] {
        setAllOff();
        updateStatus();
    });
    connect(core, &CoreConnection::simStopped, this, [this] {
        setAllOff();
        updateStatus();
    });
    connect(core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method.startsWith(u"circuit."))
            m_status->setText(tr("Could not save the parts: %1").arg(message));
    });
    connect(core, &CoreConnection::stateChanged, this, [this] {
        if (m_core->state() == CoreConnection::State::Connected && !m_project.isEmpty())
            m_core->loadCircuit(m_project, m_board);
    });
    m_remove->setEnabled(false);
    m_addLed->setEnabled(false);
    m_addButton->setEnabled(false);
    m_status->setText(tr("Select a project to place virtual parts on its board."));
}

QString CircuitPanel::statusText() const
{
    return m_status->text();
}

bool CircuitPanel::isOn(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i && i->isOn();
}

QPointF CircuitPanel::partCentre(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i ? i->mapToScene(QPointF(PartSize / 2, PartSize / 2)) : QPointF();
}

void CircuitPanel::showBoard(const QString &project, const QString &board, const QJsonArray &pins)
{
    m_project = project;
    m_board = board;
    m_projectPins.clear();
    for (const QJsonValue &v : pins) {
        bool ok = false;
        const int gpio = v.toString().toInt(&ok);
        if (ok && gpio >= 0 && gpio <= 48)
            m_projectPins << gpio;
    }
    m_circuit = {};
    rebuildScene();
    m_addLed->setEnabled(true);
    m_addButton->setEnabled(true);
    if (m_core->state() == CoreConnection::State::Connected)
        m_core->loadCircuit(project, board);
}

void CircuitPanel::circuitLoaded(const QString &project, const QString &board, const QJsonObject &circuit)
{
    if (project != m_project || board != m_board)
        return; // an answer for a board no longer shown
    m_circuit = circuit;
    rebuildScene();
    updateStatus();
}

void CircuitPanel::updateStatus()
{
    if (m_project.isEmpty())
        return;
    if (parts().isEmpty()) {
        m_status->setText(tr("No parts on this board yet. Add an LED or a button on one of its pins."));
    } else if (m_core->isSimRunning()) {
        m_status->setText(m_core->simPartsActive()
                              ? tr("Running: LEDs follow the firmware; hold the mouse on a button to press it.")
                              : tr("These parts do not work in this simulation: it has no pin access "
                                   "(ESP32-S3, or lcgamboa's QEMU not installed)."));
    } else {
        m_status->setText(tr("Parts take effect the next time the project runs in the simulator."));
    }
}

void CircuitPanel::setAllOff()
{
    for (PartItem *i : std::as_const(m_items))
        i->setOn(false);
}

void CircuitPanel::rebuildScene()
{
    m_scene->clear();
    m_items.clear();
    if (m_project.isEmpty())
        return;
    // Board pins: the project's own, then any others parts use.
    QList<int> pins = m_projectPins;
    for (const QJsonValue &v : parts()) {
        const int gpio = v.toObject().value(u"pin"_s).toInt();
        if (!pins.contains(gpio))
            pins << gpio;
    }
    std::sort(pins.begin(), pins.end());
    const QPalette pal = palette();
    const qreal height = std::max<qreal>(1, pins.size()) * RowHeight + 20;
    auto *board = m_scene->addRect(0, 0, BoardWidth, height, QPen(pal.color(QPalette::Text)),
                                   QBrush(pal.color(QPalette::AlternateBase)));
    auto *title = m_scene->addSimpleText(m_board);
    title->setBrush(pal.color(QPalette::Text));
    title->setPos(8, -24);
    Q_UNUSED(board);

    QHash<int, int> partsOnPin; // to stack several parts on one pin
    for (int row = 0; row < pins.size(); ++row) {
        const qreal y = 10 + row * RowHeight + RowHeight / 2;
        auto *label = m_scene->addSimpleText(tr("GPIO %1").arg(pins[row]));
        label->setBrush(pal.color(QPalette::Text));
        label->setPos(BoardWidth - 62, y - 9);
        m_scene->addEllipse(BoardWidth - 6, y - 4, 8, 8, QPen(pal.color(QPalette::Text)),
                            QBrush(pal.color(QPalette::Text)));
    }
    for (const QJsonValue &v : parts()) {
        const QJsonObject o = v.toObject();
        const QString id = o.value(u"id"_s).toString();
        const QString kind = o.value(u"kind"_s).toString();
        const int gpio = o.value(u"pin"_s).toInt();
        const int row = int(pins.indexOf(gpio));
        const int stack = partsOnPin[gpio]++;
        const qreal y = 10 + row * RowHeight + RowHeight / 2;
        QString label = o.value(u"label"_s).toString();
        if (label.isEmpty())
            label = id; // ids already say what they are: led-1, button-2
        if (o.value(u"activeLow"_s).toBool())
            label = tr("%1 (active low)").arg(label);
        auto *item = new PartItem(id, kind, label, [this, id](bool pressed) {
            m_core->pressPart(id, pressed);
        });
        const qreal x = PartX + stack * 200;
        item->setPos(x, y - PartSize / 2);
        m_scene->addItem(item);
        m_scene->addLine(BoardWidth + 2, y, x, y, QPen(pal.color(QPalette::Mid), 2));
        item->setToolTip(kind == u"button" ? tr("GPIO %1. Hold the mouse here to press it.").arg(gpio)
                                           : tr("GPIO %1").arg(gpio));
        m_items.insert(id, item);
    }
    m_scene->setSceneRect(m_scene->itemsBoundingRect().adjusted(-10, -10, 10, 10));
}

QString CircuitPanel::freeId(const QString &kind) const
{
    QStringList used;
    for (const QJsonValue &v : parts())
        used << v.toObject().value(u"id"_s).toString();
    for (int n = 1;; ++n) {
        const QString id = u"%1-%2"_s.arg(kind).arg(n);
        if (!used.contains(id))
            return id;
    }
}

void CircuitPanel::addPart(const QString &kind, int pin, bool activeLow, const QString &label)
{
    QJsonObject part{{u"id"_s, freeId(kind)}, {u"kind"_s, kind}, {u"pin"_s, pin}, {u"activeLow"_s, activeLow}};
    if (!label.isEmpty())
        part.insert(u"label"_s, label);
    QJsonArray list = parts();
    list.append(part);
    QJsonObject c = m_circuit;
    c.insert(u"schemaVersion"_s, c.value(u"schemaVersion"_s).toString(u"1.0.0"_s));
    c.insert(u"parts"_s, list);
    m_core->saveCircuit(m_project, m_board, c);
}

void CircuitPanel::removePart(const QString &id)
{
    QJsonArray list;
    for (const QJsonValue &v : parts()) {
        if (v.toObject().value(u"id"_s).toString() != id)
            list.append(v);
    }
    QJsonObject c = m_circuit;
    c.insert(u"schemaVersion"_s, c.value(u"schemaVersion"_s).toString(u"1.0.0"_s));
    c.insert(u"parts"_s, list);
    m_core->saveCircuit(m_project, m_board, c);
}

void CircuitPanel::addDialog(const QString &kind)
{
    QDialog dialog(this);
    dialog.setWindowTitle(kind == u"led" ? tr("Add an LED") : tr("Add a button"));
    auto *pin = new QSpinBox(&dialog);
    pin->setRange(0, 48);
    pin->setPrefix(tr("GPIO "));
    if (!m_projectPins.isEmpty())
        pin->setValue(m_projectPins.first());
    auto *activeLow = new QCheckBox(kind == u"led" ? tr("Wired to 3V3 (lit when the pin is low)")
                                                    : tr("Wired to GND with a pull-up (reads low when pressed)"),
                                    &dialog);
    activeLow->setChecked(kind == u"button"); // INPUT_PULLUP is the usual button wiring
    auto *label = new QLineEdit(&dialog);
    label->setMaxLength(40);
    label->setPlaceholderText(tr("Optional, e.g. Status LED"));
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    auto *form = new QFormLayout(&dialog);
    form->addRow(tr("Pin:"), pin);
    form->addRow(QString(), activeLow);
    form->addRow(tr("Label:"), label);
    form->addRow(buttons);
    if (dialog.exec() == QDialog::Accepted)
        addPart(kind, pin->value(), activeLow->isChecked(), label->text().trimmed());
}
