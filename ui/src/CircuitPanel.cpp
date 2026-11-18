#include "CircuitPanel.h"

#include "CoreConnection.h"

#include <QCheckBox>
#include <QComboBox>
#include <QCoreApplication>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDoubleSpinBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QGraphicsItem>
#include <QGraphicsScene>
#include <QGraphicsSceneMouseEvent>
#include <QGraphicsView>
#include <QHBoxLayout>
#include <QIcon>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QPainter>
#include <QPushButton>
#include <QSpinBox>
#include <QStyleOptionGraphicsItem>
#include <QToolButton>
#include <QVBoxLayout>

#include <algorithm>
#include <algorithm>
#include <cmath>
#include <functional>

using namespace Qt::StringLiterals;

namespace {
constexpr qreal RowHeight = 44;
constexpr qreal BoardWidth = 110;
constexpr qreal PartX = 170;
constexpr qreal PartSize = 30;
constexpr qreal StripCell = 16;
}

// One part on the canvas, drawn by its model's look from the state the core reports.
// Colours of lit parts are the canvas's own (rule: hard-coded colours only on the
// canvas); everything else follows the system palette.
class PartItem : public QGraphicsItem {
public:
    // `control(name, value)` works one of the part's controls.
    PartItem(const QString &id, const QJsonObject &model, const QString &label,
             std::function<void(const QString &, const QJsonValue &)> control)
        : m_id(id)
        , m_look(model.value(u"look"_s).toString())
        , m_label(label)
        , m_control(std::move(control))
    {
        for (const QJsonValue &c : model.value(u"controls"_s).toArray()) {
            const QJsonObject o = c.toObject();
            const QString type = o.value(u"type"_s).toString();
            if (type == u"momentary" && m_momentary.isEmpty())
                m_momentary = o.value(u"name"_s).toString();
            if (type == u"toggle" && m_toggle.isEmpty())
                m_toggle = o.value(u"name"_s).toString();
            if (type == u"slider")
                m_sliders << o;
        }
        const QJsonObject display = model.value(u"display"_s).toObject();
        m_display = QSize(display.value(u"width"_s).toInt(), display.value(u"height"_s).toInt());
        m_initial = model.value(u"state"_s).toObject();
        m_state = m_initial;
        setFlag(ItemIsSelectable);
        setAcceptedMouseButtons(Qt::LeftButton);
    }

    QString id() const { return m_id; }
    QJsonObject state() const { return m_state; }
    bool isOn() const { return m_state.value(u"on"_s).toBool(); }
    void setState(const QJsonObject &state)
    {
        if (state != m_state) {
            m_state = state;
            update();
        }
    }
    // Where the part's sliders were left (the circuit's `controls`): its state
    // before and between runs.
    void setSaved(const QJsonObject &controls)
    {
        for (auto it = controls.begin(); it != controls.end(); ++it)
            m_initial.insert(it.key(), it.value());
        setState(m_initial);
    }
    void reset()
    {
        setState(m_initial);
        setPicture({});
    }
    QImage picture() const { return m_picture; }
    void setPicture(const QImage &image)
    {
        prepareGeometryChange();
        m_picture = image;
        update();
    }
    bool isDisplay() const { return m_look == u"display" && !m_display.isEmpty(); }
    bool isBox() const { return m_look == u"box"; }
    bool isStrip() const { return m_look == u"strip"; }
    // A strip's length (its `count` property), for drawing it before the first frame.
    void setCount(int count)
    {
        prepareGeometryChange();
        m_count = std::clamp(count, 1, 64);
    }
    int stripLength() const { return std::max<int>(m_count, m_state.value(u"leds"_s).toArray().size()); }
    // Controls a person sets with a number (a sensor's readings).
    QList<QJsonObject> sliders() const { return m_sliders; }
    // A display is drawn twice its size, so single pixels read.
    QRectF screen() const { return QRectF(4, 4, m_display.width() * 2, m_display.height() * 2); }

    QRectF boundingRect() const override
    {
        if (isDisplay())
            return QRectF(-2, -2, screen().width() + 12, screen().height() + 30);
        if (isBox())
            return QRectF(-2, -2, 234, 30 + 18 * m_sliders.size());
        if (isStrip())
            return QRectF(-2, -2, StripCell * stripLength() + 4, PartSize + 22);
        return {-2, -2, PartSize + 160, PartSize + 4};
    }

    void paint(QPainter *p, const QStyleOptionGraphicsItem *option, QWidget *widget) override
    {
        p->setRenderHint(QPainter::Antialiasing);
        const QPalette pal = widget ? widget->palette() : QPalette();
        const QRectF body(0, 0, PartSize, PartSize);
        p->setPen(QPen(pal.color(QPalette::Text), 1.5));
        if (isDisplay()) {
            // The module's frame, then the glass: black, lit pixels in OLED blue-white.
            p->setBrush(QColor(0x1c, 0x2a, 0x3a));
            p->drawRoundedRect(QRectF(0, 0, screen().width() + 8, screen().height() + 8), 4, 4);
            p->fillRect(screen(), Qt::black);
            if (!m_picture.isNull()) {
                QImage lit(m_picture.size(), QImage::Format_ARGB32);
                lit.fill(Qt::transparent);
                for (int y = 0; y < m_picture.height(); ++y) {
                    const uchar *row = m_picture.constScanLine(y);
                    for (int x = 0; x < m_picture.width(); ++x)
                        if (row[x])
                            lit.setPixel(x, y, qRgb(0xa8, 0xe0, 0xff));
                }
                p->drawImage(screen(), lit); // scaled without smoothing: crisp pixels
            }
            if (option->state & QStyle::State_Selected) {
                p->setPen(QPen(pal.color(QPalette::Highlight), 1, Qt::DashLine));
                p->setBrush(Qt::NoBrush);
                p->drawRect(boundingRect().adjusted(1, 1, -1, -24));
            }
            p->setPen(pal.color(QPalette::Text));
            p->drawText(QRectF(0, screen().height() + 10, screen().width() + 8, 18), Qt::AlignLeft | Qt::AlignVCenter,
                        m_label);
            return;
        }
        if (isBox()) {
            // A breakout board: its name, then one line per reading.
            const QRectF board = boundingRect().adjusted(2, 2, -2, -2);
            p->setBrush(pal.color(QPalette::Button));
            p->drawRoundedRect(board, 4, 4);
            if (option->state & QStyle::State_Selected) {
                p->setPen(QPen(pal.color(QPalette::Highlight), 1, Qt::DashLine));
                p->setBrush(Qt::NoBrush);
                p->drawRect(board.adjusted(-2, -2, 2, 2));
            }
            p->setPen(pal.color(QPalette::ButtonText));
            QFont bold = p->font();
            bold.setBold(true);
            p->setFont(bold);
            p->drawText(QRectF(8, 4, 220, 18), Qt::AlignLeft | Qt::AlignVCenter, m_label);
            p->setFont(QFont());
            int line = 0;
            for (const QJsonObject &c : m_sliders) {
                const QString name = c.value(u"name"_s).toString();
                const double v = m_state.value(name).toDouble(c.value(u"default"_s).toDouble());
                p->drawText(QRectF(8, 24 + 18 * line++, 220, 18), Qt::AlignLeft | Qt::AlignVCenter,
                            QCoreApplication::translate("PartItem", "%1: %2 %3").arg(c.value(u"label"_s).toString()).arg(v, 0, 'f', 2)
                                .arg(c.value(u"unit"_s).toString()));
            }
            return;
        }
        if (isStrip()) {
            // The strip's backing, then one LED per cell in its colour. Brightness is
            // shown on a square-root scale, so the dim colours sketches use read.
            const int n = stripLength();
            const QJsonArray leds = m_state.value(u"leds"_s).toArray();
            p->setBrush(QColor(0x22, 0x22, 0x22));
            p->drawRoundedRect(QRectF(0, 0, StripCell * n, PartSize), 4, 4);
            p->setPen(Qt::NoPen);
            for (int i = 0; i < n; ++i) {
                const QJsonArray rgb = leds.at(i).toArray();
                auto level = [&](int k) {
                    const double v = std::clamp(rgb.at(k).toInt(), 0, 255) / 255.0;
                    return int(std::lround(255 * std::sqrt(v)));
                };
                const bool lit = rgb.size() == 3 && (rgb.at(0).toInt() || rgb.at(1).toInt() || rgb.at(2).toInt());
                p->setBrush(lit ? QColor(level(0), level(1), level(2)) : QColor(0x44, 0x44, 0x44));
                p->drawEllipse(QRectF(StripCell * i + 2, PartSize / 2 - (StripCell - 4) / 2.0, StripCell - 4, StripCell - 4));
            }
            if (option->state & QStyle::State_Selected) {
                p->setPen(QPen(pal.color(QPalette::Highlight), 1, Qt::DashLine));
                p->setBrush(Qt::NoBrush);
                p->drawRect(QRectF(-2, -2, StripCell * n + 4, PartSize + 4));
            }
            p->setPen(pal.color(QPalette::Text));
            p->drawText(QRectF(0, PartSize + 2, StripCell * n, 18), Qt::AlignLeft | Qt::AlignVCenter, m_label);
            return;
        }
        if (m_look == u"led") {
            p->setBrush(isOn() ? QColor(0xe5, 0x39, 0x35) : QColor(0x5a, 0x22, 0x22));
            p->drawEllipse(body);
            if (isOn()) { // a glow, so lit reads at a glance
                p->setPen(Qt::NoPen);
                p->setBrush(QColor(0xff, 0x8a, 0x80, 120));
                p->drawEllipse(body.adjusted(6, 6, -6, -6));
            }
        } else if (m_look == u"rgb-led") {
            const bool r = m_state.value(u"red"_s).toBool();
            const bool g = m_state.value(u"green"_s).toBool();
            const bool b = m_state.value(u"blue"_s).toBool();
            const QColor lit(r ? 0xff : 0x30, g ? 0xe0 : 0x30, b ? 0xff : 0x30);
            p->setBrush((r || g || b) ? lit : QColor(0x3a, 0x3a, 0x3a));
            p->drawEllipse(body);
        } else if (m_look == u"switch") {
            const QRectF track(0, PartSize / 4, PartSize, PartSize / 2);
            p->setBrush(pal.color(isOn() ? QPalette::Highlight : QPalette::Button));
            p->drawRoundedRect(track, PartSize / 4, PartSize / 4);
            p->setBrush(pal.color(QPalette::Base));
            const qreal knob = PartSize / 2;
            p->drawEllipse(QRectF(isOn() ? PartSize - knob : 0, PartSize / 4, knob, knob));
        } else if (m_look == u"button") {
            p->setBrush(pal.color(isOn() ? QPalette::Highlight : QPalette::Button));
            p->drawRoundedRect(body, 6, 6);
            p->setBrush(pal.color(isOn() ? QPalette::HighlightedText : QPalette::Mid));
            p->drawEllipse(body.adjusted(8, 8, -8, -8));
        } else {
            p->setBrush(pal.color(QPalette::Button));
            p->drawRect(body);
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
        if (!m_momentary.isEmpty())
            m_control(m_momentary, true);
        else if (!m_toggle.isEmpty())
            m_control(m_toggle, !isOn());
    }
    void mouseReleaseEvent(QGraphicsSceneMouseEvent *event) override
    {
        QGraphicsItem::mouseReleaseEvent(event);
        if (!m_momentary.isEmpty())
            m_control(m_momentary, false);
    }

private:
    QString m_id;
    QString m_look;
    QString m_label;
    QSize m_display;
    QImage m_picture;
    QList<QJsonObject> m_sliders;
    int m_count = 0;
    QString m_momentary;
    QString m_toggle;
    QJsonObject m_initial;
    QJsonObject m_state;
    std::function<void(const QString &, const QJsonValue &)> m_control;
};

CircuitPanel::CircuitPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(this))
    , m_add(new QToolButton(this))
    , m_remove(new QPushButton(QIcon::fromTheme(u"list-remove"_s), tr("Remove"), this))
    , m_scene(new QGraphicsScene(this))
    , m_view(new QGraphicsView(m_scene, this))
    , m_controls(new QGroupBox(this))
{
    m_status->setWordWrap(true);
    m_view->setRenderHint(QPainter::Antialiasing);
    m_view->setAlignment(Qt::AlignLeft | Qt::AlignTop);
    m_add->setText(tr("Add part"));
    m_add->setIcon(QIcon::fromTheme(u"list-add"_s));
    m_add->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_add->setPopupMode(QToolButton::InstantPopup);
    m_add->setMenu(new QMenu(m_add));
    m_add->setToolTip(tr("Place a part on the board's pins"));
    m_remove->setToolTip(tr("Remove the selected part"));
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
    connect(m_scene, &QGraphicsScene::selectionChanged, m_remove, [this] {
        m_remove->setEnabled(!m_scene->selectedItems().isEmpty());
        rebuildControls();
    });

    auto *tools = new QHBoxLayout;
    tools->addWidget(m_add);
    tools->addWidget(m_remove);
    tools->addStretch(1);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_status);
    layout->addLayout(tools);
    layout->addWidget(m_view, 1);
    m_controls->setVisible(false);
    layout->addWidget(m_controls);

    connect(core, &CoreConnection::circuitLoaded, this, &CircuitPanel::circuitLoaded);
    connect(core, &CoreConnection::modelsLoaded, this, &CircuitPanel::modelsLoaded);
    connect(core, &CoreConnection::simPartsChanged, this, [this](const QJsonArray &parts) {
        for (const QJsonValue &v : parts) {
            const QJsonObject o = v.toObject();
            const QString id = o.value(u"id"_s).toString();
            if (o.contains(u"error"_s)) {
                m_status->setText(tr("%1: %2").arg(id, o.value(u"error"_s).toString()));
                continue;
            }
            if (PartItem *i = m_items.value(id))
                i->setState(o.value(u"state"_s).toObject());
        }
    });
    connect(core, &CoreConnection::partPicture, this, [this](const QString &id, const QImage &image) {
        if (PartItem *i = m_items.value(id))
            i->setPicture(image);
    });
    connect(core, &CoreConnection::simStarted, this, [this] {
        resetStates();
        updateStatus();
        rebuildControls();
    });
    connect(core, &CoreConnection::simStopped, this, [this] {
        resetStates();
        updateStatus();
        rebuildControls();
        // The core kept the sliders moved during the run: show where they were left.
        if (!m_project.isEmpty())
            m_core->loadCircuit(m_project, m_board);
    });
    connect(core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method.startsWith(u"circuit."))
            m_status->setText(tr("Could not save the parts: %1").arg(message));
    });
    connect(core, &CoreConnection::stateChanged, this, [this] {
        if (m_core->state() != CoreConnection::State::Connected)
            return;
        m_core->loadModels();
        if (!m_project.isEmpty())
            m_core->loadCircuit(m_project, m_board);
    });
    m_remove->setEnabled(false);
    m_add->setEnabled(false);
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

QJsonObject CircuitPanel::partState(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i ? i->state() : QJsonObject();
}

int CircuitPanel::partStripLength(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i && i->isStrip() ? i->stripLength() : 0;
}

QImage CircuitPanel::partPicture(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i ? i->picture() : QImage();
}

QString CircuitPanel::partToolTip(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i ? i->toolTip() : QString();
}

QPointF CircuitPanel::partCentre(const QString &id) const
{
    const PartItem *i = m_items.value(id);
    return i ? i->mapToScene(QPointF(PartSize / 2, PartSize / 2)) : QPointF();
}

void CircuitPanel::modelsLoaded(const QJsonArray &models)
{
    m_models.clear();
    m_modelOrder.clear();
    QMenu *menu = m_add->menu();
    menu->clear();
    for (const QJsonValue &v : models) {
        const QJsonObject m = v.toObject();
        const QString id = m.value(u"id"_s).toString();
        m_models.insert(id, m);
        m_modelOrder << id;
        // Model names come from the core's model files.
        menu->addAction(m.value(u"name"_s).toString(), this, [this, id] { addDialog(id); });
    }
    m_add->setEnabled(!m_project.isEmpty() && !m_models.isEmpty());
    rebuildScene();
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
    m_add->setEnabled(!m_models.isEmpty());
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
        m_status->setText(tr("No parts on this board yet. Add an LED, a button or another part on its pins."));
    } else if (m_core->isSimRunning()) {
        m_status->setText(m_core->simPartsActive()
                              ? tr("Running: parts follow the firmware. Hold the mouse on a button to press it; "
                                   "click a switch to flip it.")
                              : tr("These parts do not work in this simulation: it has no pin access "
                                   "(ESP32-S3, or lcgamboa's QEMU not installed)."));
    } else {
        m_status->setText(tr("Parts take effect the next time the project runs in the simulator."));
    }
}

void CircuitPanel::resetStates()
{
    for (PartItem *i : std::as_const(m_items))
        i->reset();
}

// A part's GPIO connections: `pins`, or the one-pin shorthand `pin`.
static QHash<QString, int> connections(const QJsonObject &part, const QJsonObject &model)
{
    QHash<QString, int> out;
    const QJsonObject pins = part.value(u"pins"_s).toObject();
    for (auto it = pins.begin(); it != pins.end(); ++it)
        out.insert(it.key(), it.value().toInt());
    const QJsonArray names = model.value(u"pins"_s).toArray();
    if (part.contains(u"pin"_s) && names.size() == 1)
        out.insert(names.first().toString(), part.value(u"pin"_s).toInt());
    return out;
}

void CircuitPanel::rebuildScene()
{
    // Keep the selection across a rebuild (saving a control reloads the circuit).
    QStringList selected;
    for (QGraphicsItem *i : m_scene->selectedItems()) {
        if (auto *p = dynamic_cast<PartItem *>(i))
            selected << p->id();
    }
    m_scene->clear();
    m_items.clear();
    if (m_project.isEmpty())
        return;
    // Board pins: the project's own, then any others parts use.
    QList<int> pins = m_projectPins;
    for (const QJsonValue &v : parts()) {
        const QJsonObject part = v.toObject();
        for (int gpio : connections(part, m_models.value(part.value(u"kind"_s).toString())))
            if (!pins.contains(gpio))
                pins << gpio;
    }
    std::sort(pins.begin(), pins.end());
    const QPalette pal = palette();
    const qreal height = std::max<qreal>(1, pins.size()) * RowHeight + 20;
    m_scene->addRect(0, 0, BoardWidth, height, QPen(pal.color(QPalette::Text)), QBrush(pal.color(QPalette::AlternateBase)));
    auto *title = m_scene->addSimpleText(m_board);
    title->setBrush(pal.color(QPalette::Text));
    title->setPos(8, -24);
    const auto rowY = [&](int gpio) { return 10 + pins.indexOf(gpio) * RowHeight + RowHeight / 2; };

    for (int gpio : pins) {
        const qreal y = rowY(gpio);
        auto *label = m_scene->addSimpleText(tr("GPIO %1").arg(gpio));
        label->setBrush(pal.color(QPalette::Text));
        label->setPos(BoardWidth - 62, y - 9);
        m_scene->addEllipse(BoardWidth - 6, y - 4, 8, 8, QPen(pal.color(QPalette::Text)), QBrush(pal.color(QPalette::Text)));
    }
    QHash<int, int> partsOnRow; // to stack several parts on one row
    qreal busY = height + 30;   // I2C parts go below the board, one under another
    for (const QJsonValue &v : parts()) {
        const QJsonObject o = v.toObject();
        const QString id = o.value(u"id"_s).toString();
        const QJsonObject model = m_models.value(o.value(u"kind"_s).toString());
        const QHash<QString, int> conn = connections(o, model);
        if (o.contains(u"i2c"_s)) {
            const QJsonObject i2c = o.value(u"i2c"_s).toObject();
            QString where = tr("I2C bus %1, address 0x%2")
                                .arg(i2c.value(u"bus"_s).toInt())
                                .arg(i2c.value(u"address"_s).toInt(), 2, 16, QLatin1Char('0'));
            // Other connections (an RTC's alarm output) are named rather than wired.
            for (const QJsonValue &n : model.value(u"pins"_s).toArray()) {
                if (conn.contains(n.toString()))
                    where = tr("%1, %2 on GPIO %3").arg(where, n.toString().toUpper()).arg(conn.value(n.toString()));
            }
            QString label = o.value(u"label"_s).toString();
            label = label.isEmpty() ? tr("%1, %2").arg(id, where) : tr("%1, %2").arg(label, where);
            auto *item = new PartItem(id, model, label, [this, id](const QString &control, const QJsonValue &value) {
                m_core->controlPart(id, control, value);
            });
            item->setPos(0, busY);
            item->setToolTip(where);
            m_scene->addItem(item);
            busY += item->boundingRect().height() + 16;
            m_items.insert(id, item);
            continue;
        }
        // The part sits on the row of its first pin, in model order.
        int anchor = conn.isEmpty() ? (pins.isEmpty() ? 0 : pins.first()) : *std::min_element(conn.begin(), conn.end());
        for (const QJsonValue &n : model.value(u"pins"_s).toArray()) {
            if (conn.contains(n.toString())) {
                anchor = conn.value(n.toString());
                break;
            }
        }
        const int stack = partsOnRow[anchor]++;
        const qreal y = rowY(anchor);
        QString label = o.value(u"label"_s).toString();
        if (label.isEmpty())
            label = id; // ids already say what they are: led-1, button-2
        auto *item = new PartItem(id, model, label, [this, id](const QString &control, const QJsonValue &value) {
            m_core->controlPart(id, control, value);
        });
        if (item->isStrip()) {
            const QJsonObject countProp = model.value(u"props"_s).toObject().value(u"count"_s).toObject();
            item->setCount(o.value(u"props"_s).toObject().value(u"count"_s).toInt(countProp.value(u"default"_s).toInt(8)));
        }
        const qreal x = PartX + stack * 200;
        item->setPos(x, y - PartSize / 2);
        m_scene->addItem(item);
        // A wire from each connected pin to the part.
        QStringList where;
        for (auto it = conn.begin(); it != conn.end(); ++it) {
            m_scene->addLine(BoardWidth + 2, rowY(it.value()), x, y, QPen(pal.color(QPalette::Mid), 2));
            where << (conn.size() == 1 ? tr("GPIO %1").arg(it.value()) : tr("%1: GPIO %2").arg(it.key()).arg(it.value()));
        }
        item->setToolTip(where.join(u", "_s));
        m_items.insert(id, item);
    }
    for (const QJsonValue &v : parts()) {
        const QJsonObject o = v.toObject();
        if (PartItem *i = m_items.value(o.value(u"id"_s).toString()))
            i->setSaved(o.value(u"controls"_s).toObject());
    }
    m_scene->setSceneRect(m_scene->itemsBoundingRect().adjusted(-10, -10, 10, 10));
    for (const QString &id : std::as_const(selected))
        selectPart(id, true);
}

// Stopped: keep a slider's value in the circuit, where the next run starts.
void CircuitPanel::saveControl(const QString &id, const QString &name, double value)
{
    QJsonArray list = parts();
    for (qsizetype n = 0; n < list.size(); ++n) {
        QJsonObject part = list.at(n).toObject();
        if (part.value(u"id"_s).toString() != id)
            continue;
        QJsonObject controls = part.value(u"controls"_s).toObject();
        controls.insert(name, value);
        part.insert(u"controls"_s, controls);
        list.replace(n, part);
    }
    QJsonObject c = m_circuit;
    c.insert(u"schemaVersion"_s, u"1.2.0"_s);
    c.insert(u"parts"_s, list);
    m_core->saveCircuit(m_project, m_board, c);
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

void CircuitPanel::addPart(const QString &kind, const QJsonObject &pins, const QJsonObject &props, const QString &label,
                           const QJsonObject &i2c)
{
    QJsonObject part{{u"id"_s, freeId(kind)}, {u"kind"_s, kind}};
    if (!i2c.isEmpty())
        part.insert(u"i2c"_s, i2c);
    if (!pins.isEmpty())
        part.insert(u"pins"_s, pins);
    if (!props.isEmpty())
        part.insert(u"props"_s, props);
    if (!label.isEmpty())
        part.insert(u"label"_s, label);
    QJsonArray list = parts();
    list.append(part);
    QJsonObject c = m_circuit;
    c.insert(u"schemaVersion"_s, u"1.2.0"_s);
    c.insert(u"parts"_s, list);
    m_core->saveCircuit(m_project, m_board, c);
}

void CircuitPanel::addPart(const QString &kind, int pin, bool activeLow, const QString &label)
{
    const QJsonArray names = m_models.value(kind).value(u"pins"_s).toArray();
    const QString pinName = names.isEmpty() ? u"pin"_s : names.first().toString();
    addPart(kind, QJsonObject{{pinName, pin}}, QJsonObject{{u"activeLow"_s, activeLow}}, label);
}

void CircuitPanel::removePart(const QString &id)
{
    QJsonArray list;
    for (const QJsonValue &v : parts()) {
        if (v.toObject().value(u"id"_s).toString() != id)
            list.append(v);
    }
    QJsonObject c = m_circuit;
    c.insert(u"schemaVersion"_s, u"1.2.0"_s);
    c.insert(u"parts"_s, list);
    m_core->saveCircuit(m_project, m_board, c);
}

// A dialog built from the model: a GPIO for each pin, a field for each property.
void CircuitPanel::addDialog(const QString &kind)
{
    const QJsonObject model = m_models.value(kind);
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Add %1").arg(model.value(u"name"_s).toString()));
    auto *form = new QFormLayout(&dialog);
    QList<QPair<QString, QSpinBox *>> pinBoxes;
    int next = 0;
    const QJsonArray optional = model.value(u"optionalPins"_s).toArray();
    for (const QJsonValue &v : model.value(u"pins"_s).toArray()) {
        auto *box = new QSpinBox(&dialog);
        box->setPrefix(tr("GPIO "));
        if (optional.contains(v)) {
            // -1 is "not connected", the default for a pin that may be left free.
            box->setRange(-1, 48);
            box->setSpecialValueText(tr("Not connected"));
            box->setValue(-1);
        } else {
            box->setRange(0, 48);
            if (next < m_projectPins.size())
                box->setValue(m_projectPins.at(next++));
        }
        const QString name = v.toString();
        form->addRow(model.value(u"pins"_s).toArray().size() == 1 ? tr("Pin:") : tr("%1 pin:").arg(name), box);
        pinBoxes << qMakePair(name, box);
    }
    QList<std::function<QPair<QString, QJsonValue>()>> propFields;
    const QJsonObject props = model.value(u"props"_s).toObject();
    for (auto it = props.begin(); it != props.end(); ++it) {
        const QJsonObject p = it.value().toObject();
        const QString key = it.key();
        const QString type = p.value(u"type"_s).toString();
        const QString label = p.value(u"label"_s).toString();
        if (type == u"bool") {
            auto *box = new QCheckBox(label, &dialog);
            // Buttons are usually wired to GND with INPUT_PULLUP.
            box->setChecked(kind == u"button" && key == u"activeLow" ? true : p.value(u"default"_s).toBool());
            form->addRow(QString(), box);
            propFields << [key, box] { return qMakePair(key, QJsonValue(box->isChecked())); };
        } else {
            auto *box = new QDoubleSpinBox(&dialog);
            box->setDecimals(type == u"int" ? 0 : 2);
            box->setRange(p.value(u"min"_s).toDouble(-1e9), p.value(u"max"_s).toDouble(1e9));
            box->setValue(p.value(u"default"_s).toDouble());
            form->addRow(label, box);
            propFields << [key, box, type] {
                return qMakePair(key, type == u"int" ? QJsonValue(qRound(box->value())) : QJsonValue(box->value()));
            };
        }
    }
    QComboBox *address = nullptr;
    QSpinBox *bus = nullptr;
    const QJsonArray addresses = model.value(u"i2c"_s).toObject().value(u"addresses"_s).toArray();
    if (!addresses.isEmpty()) {
        bus = new QSpinBox(&dialog);
        bus->setRange(0, 1);
        bus->setToolTip(tr("Wire uses bus 0"));
        form->addRow(tr("I2C bus:"), bus);
        address = new QComboBox(&dialog);
        for (const QJsonValue &a : addresses)
            address->addItem(u"0x%1"_s.arg(a.toInt(), 2, 16, QLatin1Char('0')), a.toInt());
        form->addRow(tr("Address:"), address);
    }
    auto *labelEdit = new QLineEdit(&dialog);
    labelEdit->setMaxLength(40);
    labelEdit->setPlaceholderText(tr("Optional, e.g. Status LED"));
    form->addRow(tr("Label:"), labelEdit);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    form->addRow(buttons);
    if (dialog.exec() != QDialog::Accepted)
        return;
    QJsonObject pins;
    for (const auto &[name, box] : std::as_const(pinBoxes)) {
        if (box->value() >= 0)
            pins.insert(name, box->value());
    }
    QJsonObject values;
    for (const auto &field : std::as_const(propFields)) {
        const auto [key, value] = field();
        values.insert(key, value);
    }
    QJsonObject i2c;
    if (address)
        i2c = {{u"bus"_s, bus->value()}, {u"address"_s, address->currentData().toInt()}};
    addPart(kind, pins, values, labelEdit->text().trimmed(), i2c);
}

QDoubleSpinBox *CircuitPanel::controlBox(const QString &name) const
{
    return m_controls->findChild<QDoubleSpinBox *>(name);
}

// The selected part's number controls (a sensor's readings), as spin boxes. They
// work while the simulation runs its parts; values are not kept between runs.
void CircuitPanel::rebuildControls()
{
    delete m_controls->layout();
    qDeleteAll(m_controls->findChildren<QWidget *>(Qt::FindDirectChildrenOnly));
    PartItem *part = nullptr;
    for (QGraphicsItem *i : m_scene->selectedItems()) {
        if ((part = dynamic_cast<PartItem *>(i)))
            break;
    }
    const QList<QJsonObject> sliders = part ? part->sliders() : QList<QJsonObject>();
    m_controls->setVisible(!sliders.isEmpty());
    if (sliders.isEmpty())
        return;
    // Running its parts: changes go to the simulation (and the core keeps them).
    // Stopped: they are saved, and the next run starts there. Running on a
    // simulator without parts: nothing to change.
    const bool live = m_core->simPartsActive();
    const bool editable = live || !m_core->isSimRunning();
    m_controls->setTitle(live       ? tr("Controls for %1").arg(part->id())
                         : editable ? tr("Controls for %1 (where the next run starts)").arg(part->id())
                                    : tr("Controls for %1 (this simulator does not run parts)").arg(part->id()));
    auto *form = new QFormLayout(m_controls);
    const QString id = part->id();
    for (const QJsonObject &c : sliders) {
        const QString name = c.value(u"name"_s).toString();
        auto *box = new QDoubleSpinBox(m_controls);
        box->setObjectName(name);
        box->setRange(c.value(u"min"_s).toDouble(), c.value(u"max"_s).toDouble());
        box->setDecimals(2);
        box->setSuffix(u" "_s + c.value(u"unit"_s).toString());
        box->setValue(part->state().value(name).toDouble(c.value(u"default"_s).toDouble()));
        box->setEnabled(editable);
        box->setKeyboardTracking(false); // one change per finished edit, not per key
        connect(box, &QDoubleSpinBox::valueChanged, this, [this, id, name](double v) {
            if (m_core->simPartsActive())
                m_core->controlPart(id, name, v);
            else if (!m_core->isSimRunning())
                saveControl(id, name, v);
        });
        form->addRow(c.value(u"label"_s).toString(), box);
    }
}

void CircuitPanel::selectPart(const QString &id, bool selected)
{
    if (PartItem *i = m_items.value(id))
        i->setSelected(selected);
}
