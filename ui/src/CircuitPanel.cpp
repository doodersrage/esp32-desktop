#include "CircuitPanel.h"

#include "Labels.h"

#include <QKeyEvent>
#include <QWheelEvent>
#include <QShortcut>

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
#include <QDir>
#include <QStandardPaths>
#include <QFileDialog>
#include <QJsonDocument>
#include <QMimeData>
#include <QGuiApplication>
#include <QClipboard>
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
        // Dragged to where the user likes it; the wires follow and the place is saved.
        setFlag(ItemIsMovable);
        // Reached with Tab too: the arrow keys move it, Space presses or toggles it.
        setFlag(ItemIsFocusable);
        setFlag(ItemSendsGeometryChanges);
        setAcceptedMouseButtons(Qt::LeftButton);
    }

    // Called as the part moves (to redraw its wires) and once dropped somewhere else.
    std::function<void(QPointF)> moving;
    std::function<void(QPointF)> moved;

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
    void focusInEvent(QFocusEvent *event) override
    {
        QGraphicsItem::focusInEvent(event);
        // The focused part is the selected one, drawn with its dashed frame.
        if (scene())
            scene()->clearSelection();
        setSelected(true);
    }
    void keyPressEvent(QKeyEvent *event) override
    {
        const qreal step = event->modifiers() & Qt::ShiftModifier ? 1 : 10;
        QPointF by;
        switch (event->key()) {
        case Qt::Key_Left: by = {-step, 0}; break;
        case Qt::Key_Right: by = {step, 0}; break;
        case Qt::Key_Up: by = {0, -step}; break;
        case Qt::Key_Down: by = {0, step}; break;
        case Qt::Key_Space:
            if (!event->isAutoRepeat()) {
                if (!m_momentary.isEmpty())
                    m_control(m_momentary, true);
                else if (!m_toggle.isEmpty())
                    m_control(m_toggle, !isOn());
            }
            return;
        default:
            QGraphicsItem::keyPressEvent(event);
            return;
        }
        if (!m_keyMoving) {
            m_keyMoving = true;
            m_pressedAt = pos();
        }
        setPos(pos() + by);
    }
    void keyReleaseEvent(QKeyEvent *event) override
    {
        if (event->isAutoRepeat())
            return;
        if (event->key() == Qt::Key_Space && !m_momentary.isEmpty()) {
            m_control(m_momentary, false);
            return;
        }
        // Saved once the key is let go, not on every step.
        if (m_keyMoving && moved && pos() != m_pressedAt)
            moved(pos());
        m_keyMoving = false;
        QGraphicsItem::keyReleaseEvent(event);
    }
    QVariant itemChange(GraphicsItemChange change, const QVariant &value) override
    {
        // Dragged with the mouse: onto a 10-pixel grid, so parts line up.
        if (change == ItemPositionChange && m_dragging) {
            const QPointF p = value.toPointF();
            return QPointF(std::round(p.x() / 10) * 10, std::round(p.y() / 10) * 10);
        }
        if (change == ItemPositionHasChanged && moving)
            moving(value.toPointF());
        return QGraphicsItem::itemChange(change, value);
    }
    void mousePressEvent(QGraphicsSceneMouseEvent *event) override
    {
        m_pressedAt = pos();
        m_dragging = true;
        QGraphicsItem::mousePressEvent(event);
        if (!m_momentary.isEmpty())
            m_control(m_momentary, true);
        else if (!m_toggle.isEmpty())
            m_control(m_toggle, !isOn());
    }
    void mouseReleaseEvent(QGraphicsSceneMouseEvent *event) override
    {
        QGraphicsItem::mouseReleaseEvent(event);
        m_dragging = false;
        if (moved && (pos() - m_pressedAt).manhattanLength() > 4)
            moved(pos());
        if (!m_momentary.isEmpty())
            m_control(m_momentary, false);
    }

private:
    QPointF m_pressedAt;
    bool m_keyMoving = false;
    bool m_dragging = false;
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
    , m_addSuggested(new QPushButton(QIcon::fromTheme(u"list-add"_s), tr("Add the project's parts"), this))
    , m_undoButton(new QPushButton(QIcon::fromTheme(u"edit-undo"_s), tr("Undo"), this))
    , m_scene(new QGraphicsScene(this))
    , m_view(new QGraphicsView(m_scene, this))
    , m_controls(new QGroupBox(this))
{
    m_view->setAccessibleName(tr("Circuit: the board and its parts"));
    m_view->setAccessibleDescription(tr("Tab moves between parts; the arrow keys move the selected one (with Shift, in small steps), and Space presses or switches it."));
    m_status->setWordWrap(true);
    m_view->setFocusPolicy(Qt::StrongFocus);
    // Tab and Shift+Tab step through the parts, in the circuit's order, then on out of
    // the canvas (a scene's own Tab order covers only QGraphicsWidgets).
    m_view->installEventFilter(this);
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
        if (const QString id = selectedPartId(); !id.isEmpty())
            removePart(id);
    });
    m_duplicate = new QPushButton(QIcon::fromTheme(u"edit-copy"_s), tr("Duplicate"), this);
    m_duplicate->setToolTip(tr("Add another part like the selected one, on pins it does not use (Ctrl+D)"));
    m_duplicate->setEnabled(false);
    connect(m_duplicate, &QPushButton::clicked, this, [this] {
        const QString id = selectedPartId();
        for (const QJsonValue &v : parts()) {
            const QJsonObject part = v.toObject();
            if (part.value(u"id"_s).toString() == id) {
                addDialog(part.value(u"kind"_s).toString(), part);
                return;
            }
        }
    });
    m_paste = new QPushButton(QIcon::fromTheme(u"edit-paste"_s), tr("Paste"), this);
    m_paste->setToolTip(tr("Add the part copied with Ctrl+C, from this or another project, on pins it does not "
                           "use here (Ctrl+V)"));
    m_paste->setEnabled(false);
    connect(m_paste, &QPushButton::clicked, this, &CircuitPanel::pastePart);
    connect(QGuiApplication::clipboard(), &QClipboard::dataChanged, this, &CircuitPanel::updatePaste);
    // m_remove as the context: the scene emits selectionChanged while it is destroyed,
    // after the button has gone.
    connect(m_scene, &QGraphicsScene::selectionChanged, m_remove, [this] {
        m_remove->setEnabled(!m_scene->selectedItems().isEmpty());
        m_duplicate->setEnabled(!selectedPartId().isEmpty());
        if (m_examples)
            m_examples->setEnabled(!selectedPartId().isEmpty());
        rebuildControls();
    });

    m_addSuggested->setVisible(false);
    connect(m_addSuggested, &QPushButton::clicked, this, [this] {
        if (!m_project.isEmpty() && !m_suggested.value(u"parts"_s).toArray().isEmpty())
            edit(m_suggested);
    });
    connect(core, &CoreConnection::circuitSuggested, this, &CircuitPanel::circuitSuggested);

    m_undoButton->setToolTip(tr("Undo the last change to the parts (Ctrl+Z)"));
    m_undoButton->setShortcut(QKeySequence::Undo);
    m_undoButton->setEnabled(false);
    connect(m_undoButton, &QPushButton::clicked, this, &CircuitPanel::undo);
    m_redoButton = new QPushButton(QIcon::fromTheme(u"edit-redo"_s), tr("Redo"), this);
    m_redoButton->setToolTip(tr("Make the change Undo took back again (%1)")
                                 .arg(QKeySequence(QKeySequence::Redo).toString(QKeySequence::NativeText)));
    m_redoButton->setShortcut(QKeySequence::Redo);
    m_redoButton->setEnabled(false);
    connect(m_redoButton, &QPushButton::clicked, this, &CircuitPanel::redo);

    auto *tools = new QHBoxLayout;
    tools->addWidget(m_addSuggested);
    tools->addWidget(m_add);
    tools->addWidget(m_remove);
    tools->addWidget(m_duplicate);
    tools->addWidget(m_paste);
    m_copyPicture = new QToolButton(this);
    m_copyPicture->setIcon(QIcon::fromTheme(u"edit-copy"_s));
    m_copyPicture->setText(tr("Copy picture"));
    m_copyPicture->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_copyPicture->setToolTip(tr("Copy the circuit as a picture, to show its wiring in a question or a README"));
    connect(m_copyPicture, &QToolButton::clicked, this, [this] {
        const QImage image = picture();
        if (!image.isNull()) {
            QGuiApplication::clipboard()->setImage(image);
            m_status->setText(tr("Copied the circuit as a picture."));
        }
    });
    tools->addWidget(m_undoButton);
    tools->addWidget(m_redoButton);
    // Zoom: Ctrl+wheel or Ctrl+= / Ctrl+- on the canvas, and Fit for all of it.
    m_fitButton = new QToolButton(this);
    m_fitButton->setIcon(QIcon::fromTheme(u"zoom-fit-best"_s));
    m_fitButton->setText(tr("Fit"));
    m_fitButton->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_fitButton->setToolTip(tr("Show the whole circuit (Ctrl+0); Ctrl and the mouse wheel zoom"));
    connect(m_fitButton, &QToolButton::clicked, this, &CircuitPanel::fitView);
    tools->addWidget(m_fitButton);
    tools->addWidget(m_copyPicture);
    m_examples = new QToolButton(this);
    m_examples->setText(tr("Examples"));
    m_examples->setIcon(QIcon::fromTheme(u"folder-open"_s));
    m_examples->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_examples->setToolTip(tr("Library projects that use the selected part, to try it with"));
    m_examples->setEnabled(false);
    connect(m_examples, &QToolButton::clicked, this, [this] {
        const QString id = selectedPartId();
        for (const QJsonValue &v : parts()) {
            if (v.toObject().value(u"id"_s).toString() == id) {
                m_examplesFor = v.toObject().value(u"kind"_s).toString();
                m_core->circuitExamples(m_examplesFor);
            }
        }
    });
    connect(core, &CoreConnection::examplesListed, this, [this](const QString &kind, const QJsonArray &projects) {
        if (kind != std::exchange(m_examplesFor, {}))
            return;
        QMenu *menu = examplesMenu(kind, projects);
        menu->setAttribute(Qt::WA_DeleteOnClose);
        menu->popup(m_examples->mapToGlobal(QPoint(0, m_examples->height())));
    });
    tools->addWidget(m_examples);
    m_tidy = new QToolButton(this);
    m_tidy->setText(tr("Tidy"));
    m_tidy->setIcon(QIcon::fromTheme(u"view-restore"_s));
    m_tidy->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_tidy->setToolTip(tr("Put every part back in its place beside the board (Undo puts them where they were)"));
    connect(m_tidy, &QToolButton::clicked, this, &CircuitPanel::tidy);
    tools->addWidget(m_tidy);
    // The parts to a file and back, to share a wiring setup.
    m_file = new QToolButton(this);
    m_file->setText(tr("File"));
    m_file->setIcon(QIcon::fromTheme(u"document-save"_s));
    m_file->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_file->setPopupMode(QToolButton::InstantPopup);
    m_file->setToolTip(tr("Save the parts to a file, or put a file's parts on this board"));
    auto *fileMenu = new QMenu(m_file);
    fileMenu->addAction(tr("Export circuit…"), this, [this] {
        const QString path = QFileDialog::getSaveFileName(
            this, tr("Export the circuit"),
            QDir(QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation))
                .filePath(u"%1-%2-circuit.json"_s.arg(m_project, m_board)),
            tr("Circuit files (*.json)"));
        if (!path.isEmpty())
            exportTo(path);
    });
    fileMenu->addAction(tr("Import circuit…"), this, [this] {
        const QString path = QFileDialog::getOpenFileName(
            this, tr("Import a circuit"), QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation),
            tr("Circuit files (*.json)"));
        if (!path.isEmpty())
            importFrom(path);
    });
    m_file->setMenu(fileMenu);
    connect(core, &CoreConnection::circuitExported, this, [this](const QString &path, int) {
        m_status->setText(tr("Exported the circuit to %1.").arg(path));
    });
    tools->addWidget(m_file);
    const auto key = [this](const QKeySequence &seq, auto slot) {
        auto *s = new QShortcut(seq, m_view, slot);
        s->setContext(Qt::WidgetWithChildrenShortcut);
    };
    key(QKeySequence::ZoomIn, [this] { zoomBy(1.25); });
    key(QKeySequence(Qt::CTRL | Qt::Key_Equal), [this] { zoomBy(1.25); });
    key(QKeySequence::ZoomOut, [this] { zoomBy(0.8); });
    key(QKeySequence(Qt::CTRL | Qt::Key_0), [this] { fitView(); });
    // On the canvas: Delete (or Backspace) removes the selected part, Ctrl+D copies it.
    key(QKeySequence::Delete, [this] { m_remove->click(); });
    key(QKeySequence(Qt::Key_Backspace), [this] { m_remove->click(); });
    key(QKeySequence(Qt::CTRL | Qt::Key_D), [this] { m_duplicate->click(); });
    key(QKeySequence::Copy, [this] { copyPart(); });
    key(QKeySequence::Paste, [this] { pastePart(); });
    m_view->viewport()->installEventFilter(this);
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
        if (method == u"circuit.set") {
            m_saving = std::max(0, m_saving - 1);
            updateUndo();
        }
        // An import that failed changed nothing: no step to undo.
        if (method == u"circuit.import" && !m_undo.isEmpty()) {
            m_undo.removeLast();
            updateUndo();
        }
    });
    connect(core, &CoreConnection::pinNotesListed, this,
            [this](const QString &board, const QString &, const QJsonArray &notes) {
                if (board != m_board)
                    return;
                m_pinNotes.clear();
                for (const QJsonValue &v : notes)
                    m_pinNotes.insert(v.toObject().value(u"gpio"_s).toInt(), v.toObject().value(u"kind"_s).toString());
                rebuildScene(); // for the pins' tooltips
            });
    connect(core, &CoreConnection::circuitSaved, this, [this] {
        m_saving = std::max(0, m_saving - 1);
        updateUndo();
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
    m_suggested = {};
    m_unmatched.clear();
    m_undo.clear();
    m_redo.clear();
    m_saving = 0;
    m_pinNotes.clear();
    if (!board.isEmpty())
        m_core->getPinNotes(board);
    updateUndo();
    m_addSuggested->setVisible(false);
    rebuildScene();
    m_add->setEnabled(!m_models.isEmpty());
    if (m_core->state() == CoreConnection::State::Connected)
        m_core->loadCircuit(project, board);
}

bool CircuitPanel::tidy()
{
    QJsonArray list = parts();
    bool moved = false;
    for (qsizetype n = 0; n < list.size(); ++n) {
        QJsonObject part = list.at(n).toObject();
        if (part.contains(u"at"_s)) {
            part.remove(u"at"_s);
            moved = true;
        }
        list.replace(n, part);
    }
    if (!moved || m_project.isEmpty())
        return false;
    QJsonObject c = m_circuit;
    c.insert(u"parts"_s, list);
    edit(c);
    return true;
}

void CircuitPanel::exportTo(const QString &path)
{
    if (!m_project.isEmpty())
        m_core->exportCircuit(m_project, m_board, path);
}

void CircuitPanel::importFrom(const QString &path)
{
    if (m_project.isEmpty())
        return;
    // An import replaces the parts: Undo puts them back.
    m_undo.append(m_circuit);
    m_redo.clear();
    updateUndo();
    m_core->importCircuit(m_project, m_board, path);
}

static QHash<QString, int> connections(const QJsonObject &part, const QJsonObject &model);

QString CircuitPanel::pinTip(int gpio) const
{
    QStringList lines{tr("GPIO %1").arg(gpio)};
    QStringList users;
    for (const QJsonValue &v : parts()) {
        const QJsonObject part = v.toObject();
        if (connections(part, m_models.value(part.value(u"kind"_s).toString())).values().contains(gpio))
            users << part.value(u"label"_s).toString(part.value(u"id"_s).toString());
    }
    lines << (users.isEmpty() ? tr("No part on it") : tr("Wired to %1").arg(users.join(u", "_s)));
    if (const QString note = pinNote(gpio); !note.isEmpty())
        lines << note;
    return lines.join(u'\n');
}

QString CircuitPanel::pinNote(int gpio) const
{
    return Labels::pinNoteText(m_pinNotes.value(gpio), gpio);
}

void CircuitPanel::circuitLoaded(const QString &project, const QString &board, const QJsonObject &circuit)
{
    if (project != m_project || board != m_board)
        return; // an answer for a board no longer shown
    m_circuit = circuit;
    rebuildScene();
    updatePaste();
    // An empty board: ask what the project's wiring has, to offer it.
    if (parts().isEmpty() && m_suggested.isEmpty())
        m_core->suggestCircuit(project, board);
    m_addSuggested->setVisible(parts().isEmpty() && !m_suggested.value(u"parts"_s).toArray().isEmpty());
    updateStatus();
}

void CircuitPanel::circuitSuggested(const QString &project, const QString &board, const QJsonObject &circuit,
                                    const QStringList &unmatched)
{
    if (project != m_project || board != m_board)
        return;
    m_suggested = circuit;
    m_unmatched = unmatched;
    QStringList names;
    for (const QJsonValue &v : circuit.value(u"parts"_s).toArray())
        names << v.toObject().value(u"label"_s).toString();
    m_addSuggested->setToolTip(names.join(u", "_s));
    m_addSuggested->setVisible(parts().isEmpty() && !names.isEmpty());
    updateStatus();
}

void CircuitPanel::updateStatus()
{
    if (m_project.isEmpty())
        return;
    // The project's parts that have no model, said once there is something to say.
    const QString missing = m_unmatched.isEmpty()
        ? QString()
        : tr("Not simulated: %1.").arg(m_unmatched.join(u", "_s));
    QStringList suggested;
    for (const QJsonValue &v : m_suggested.value(u"parts"_s).toArray())
        suggested << v.toObject().value(u"label"_s).toString();
    if (parts().isEmpty() && !suggested.isEmpty()) {
        m_status->setText(tr("The project's wiring has parts the simulator can show: %1. Add them to see them "
                             "work when it runs.")
                              .arg(suggested.join(u", "_s))
                          + (missing.isEmpty() ? QString() : u" "_s + missing));
    } else if (parts().isEmpty()) {
        m_status->setText(tr("No parts on this board yet. Add an LED, a button or another part on its pins.")
                          + (missing.isEmpty() ? QString() : u" "_s + missing));
    } else if (m_core->isSimRunning()) {
        m_status->setText(m_core->simPartsActive()
                              ? tr("Running: parts follow the firmware. Hold the mouse on a button to press it; "
                                   "click a switch to flip it.")
                              : tr("These parts do not work in this simulation: it has no pin access "
                                   "(lcgamboa's QEMU is not installed)."));
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
        auto *dot = m_scene->addEllipse(BoardWidth - 6, y - 4, 8, 8, QPen(pal.color(QPalette::Text)),
                                        QBrush(pal.color(QPalette::Text)));
        // Hovering a pin: what is on it, and what to be careful of.
        const QString tip = pinTip(gpio);
        label->setToolTip(tip);
        dot->setToolTip(tip);
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
            const QJsonObject at = o.value(u"at"_s).toObject();
            item->setPos(at.isEmpty() ? QPointF(0, busY) : QPointF(at.value(u"x"_s).toDouble(), at.value(u"y"_s).toDouble()));
            item->setToolTip(where);
            m_scene->addItem(item);
            if (at.isEmpty())
                busY += item->boundingRect().height() + 16;
            item->moved = [this, id](QPointF to) { movePart(id, to); };
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
        // Where it was moved to, else on its pin's row, after any others there.
        const QJsonObject at = o.value(u"at"_s).toObject();
        const QPointF place = at.isEmpty() ? QPointF(PartX + stack * 200, y - PartSize / 2)
                                           : QPointF(at.value(u"x"_s).toDouble(), at.value(u"y"_s).toDouble());
        item->setPos(place);
        m_scene->addItem(item);
        // A wire from each connected pin to the part; they follow it when it is dragged.
        QStringList where;
        QList<QGraphicsLineItem *> wires;
        for (auto it = conn.begin(); it != conn.end(); ++it) {
            wires << m_scene->addLine(BoardWidth + 2, rowY(it.value()), place.x(), place.y() + PartSize / 2,
                                      QPen(pal.color(QPalette::Mid), 2));
            where << (conn.size() == 1 ? tr("GPIO %1").arg(it.value()) : tr("%1: GPIO %2").arg(it.key()).arg(it.value()));
        }
        item->moving = [wires](QPointF to) {
            for (QGraphicsLineItem *w : wires) {
                QLineF l = w->line();
                l.setP2(QPointF(to.x(), to.y() + PartSize / 2));
                w->setLine(l);
            }
        };
        item->moved = [this, id](QPointF to) { movePart(id, to); };
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
    c.insert(u"schemaVersion"_s, u"1.3.0"_s);
    c.insert(u"parts"_s, list);
    save(c);
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
    c.insert(u"schemaVersion"_s, u"1.3.0"_s);
    c.insert(u"parts"_s, list);
    edit(c);
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
    c.insert(u"schemaVersion"_s, u"1.3.0"_s);
    c.insert(u"parts"_s, list);
    edit(c);
}

void CircuitPanel::movePart(const QString &id, QPointF to)
{
    QJsonArray list = parts();
    const QJsonObject at{{u"x"_s, qRound(to.x())}, {u"y"_s, qRound(to.y())}};
    for (qsizetype n = 0; n < list.size(); ++n) {
        QJsonObject part = list.at(n).toObject();
        if (part.value(u"id"_s).toString() != id)
            continue;
        // Where it already is (a release reported twice): nothing to save or undo.
        if (part.value(u"at"_s).toObject() == at)
            return;
        part.insert(u"at"_s, at);
        list.replace(n, part);
    }
    QJsonObject c = m_circuit;
    c.insert(u"schemaVersion"_s, u"1.3.0"_s);
    c.insert(u"parts"_s, list);
    edit(c);
}

void CircuitPanel::edit(const QJsonObject &circuit)
{
    // What there was before, to go back to; a few dozen steps are plenty.
    m_undo.append(m_circuit);
    if (m_undo.size() > 30)
        m_undo.removeFirst();
    m_redo.clear();
    save(circuit);
}

void CircuitPanel::updateUndo()
{
    // Not while a change is being saved: Undo goes back from the circuit as saved.
    m_undoButton->setEnabled(!m_undo.isEmpty() && m_saving == 0);
    m_redoButton->setEnabled(!m_redo.isEmpty() && m_saving == 0);
}

void CircuitPanel::save(const QJsonObject &circuit)
{
    ++m_saving;
    updateUndo();
    m_core->saveCircuit(m_project, m_board, circuit);
}

void CircuitPanel::undo()
{
    if (m_undo.isEmpty() || m_project.isEmpty() || m_saving > 0)
        return;
    QJsonObject previous = m_undo.takeLast();
    if (previous.isEmpty())
        previous = QJsonObject{{u"schemaVersion"_s, u"1.3.0"_s}, {u"parts"_s, QJsonArray{}}};
    m_redo.append(m_circuit);
    save(previous);
}

void CircuitPanel::redo()
{
    if (m_redo.isEmpty() || m_project.isEmpty() || m_saving > 0)
        return;
    QJsonObject next = m_redo.takeLast();
    if (next.isEmpty())
        next = QJsonObject{{u"schemaVersion"_s, u"1.3.0"_s}, {u"parts"_s, QJsonArray{}}};
    m_undo.append(m_circuit);
    save(next);
}

QMenu *CircuitPanel::examplesMenu(const QString &, const QJsonArray &projects)
{
    auto *menu = new QMenu(this);
    if (projects.isEmpty()) {
        menu->addAction(tr("No library project uses this part yet"))->setEnabled(false);
        return menu;
    }
    for (const QJsonValue &v : projects) {
        const QJsonObject p = v.toObject();
        const QJsonArray boards = p.value(u"boards"_s).toArray();
        // On this board if the project is wired for it, else on its first.
        QString board = boards.first().toString();
        for (const QJsonValue &b : boards)
            if (b.toString() == m_board)
                board = m_board;
        const QString id = p.value(u"id"_s).toString();
        menu->addAction(p.value(u"name"_s).toString(id), this, [this, id, board] { emit exampleRequested(id, board); });
    }
    return menu;
}

QImage CircuitPanel::picture() const
{
    const QRectF area = m_scene->itemsBoundingRect().adjusted(-16, -16, 16, 16);
    if (area.isEmpty())
        return {};
    // Twice the canvas's size, sharp enough to read when pasted.
    QImage image((area.size() * 2).toSize(), QImage::Format_ARGB32_Premultiplied);
    image.fill(palette().color(QPalette::Base));
    QPainter p(&image);
    p.setRenderHint(QPainter::Antialiasing);
    p.setRenderHint(QPainter::TextAntialiasing);
    m_scene->render(&p, QRectF(QPointF(0, 0), QSizeF(image.size())), area);
    return image;
}

bool CircuitPanel::copyPart()
{
    const QString id = selectedPartId();
    for (const QJsonValue &v : parts()) {
        QJsonObject part = v.toObject();
        if (part.value(u"id"_s).toString() != id)
            continue;
        part.remove(u"at"_s); // where it sat on this canvas
        const QByteArray json = QJsonDocument(part).toJson(QJsonDocument::Compact);
        auto *data = new QMimeData;
        data->setData(QString::fromLatin1(PartMime), json);
        data->setText(QString::fromUtf8(json));
        QGuiApplication::clipboard()->setMimeData(data);
        m_status->setText(tr("Copied %1. Paste it into this or another project's circuit with %2.")
                              .arg(id, QKeySequence(QKeySequence::Paste).toString(QKeySequence::NativeText)));
        return true;
    }
    return false;
}

bool CircuitPanel::pastePart()
{
    const QMimeData *data = QGuiApplication::clipboard()->mimeData();
    if (m_project.isEmpty() || !data || !data->hasFormat(QString::fromLatin1(PartMime)))
        return false;
    const QJsonObject part = QJsonDocument::fromJson(data->data(QString::fromLatin1(PartMime))).object();
    const QString kind = part.value(u"kind"_s).toString();
    if (!m_models.contains(kind)) {
        m_status->setText(tr("The copied part is not one this version of the app can place."));
        return false;
    }
    addDialog(kind, part);
    return true;
}

void CircuitPanel::updatePaste()
{
    const QMimeData *data = QGuiApplication::clipboard()->mimeData();
    m_paste->setEnabled(!m_project.isEmpty() && data && data->hasFormat(QString::fromLatin1(PartMime)));
}

QString CircuitPanel::selectedPartId() const
{
    for (QGraphicsItem *i : m_scene->selectedItems()) {
        if (auto *part = dynamic_cast<PartItem *>(i))
            return part->id();
    }
    return {};
}

// A dialog built from the model: a GPIO for each pin, a field for each property.
void CircuitPanel::addDialog(const QString &kind, const QJsonObject &from)
{
    const QJsonObject model = m_models.value(kind);
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Add %1").arg(model.value(u"name"_s).toString()));
    auto *form = new QFormLayout(&dialog);
    QList<QPair<QString, QSpinBox *>> pinBoxes;
    int next = 0;
    // A copy: the project's pins no part uses come first, else the original's.
    QList<int> used;
    QList<QPair<int, int>> usedAddresses; // bus, address
    for (const QJsonValue &v : parts()) {
        const QJsonObject p = v.toObject();
        used << connections(p, m_models.value(p.value(u"kind"_s).toString())).values();
        if (p.contains(u"i2c"_s)) {
            const QJsonObject i = p.value(u"i2c"_s).toObject();
            usedAddresses << qMakePair(i.value(u"bus"_s).toInt(), i.value(u"address"_s).toInt());
        }
    }
    QList<int> freePins;
    for (int gpio : std::as_const(m_projectPins))
        if (!used.contains(gpio))
            freePins << gpio;
    const QHash<QString, int> fromPins = connections(from, model);
    const QJsonObject fromProps = from.value(u"props"_s).toObject();
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
            if (!from.isEmpty()) {
                if (fromPins.contains(v.toString()))
                    box->setValue(next < freePins.size() ? freePins.at(next++) : fromPins.value(v.toString()));
            } else if (next < m_projectPins.size()) {
                box->setValue(m_projectPins.at(next++));
            }
        }
        const QString name = v.toString();
        form->addRow(model.value(u"pins"_s).toArray().size() == 1 ? tr("Pin:") : tr("%1 pin:").arg(name), box);
        pinBoxes << qMakePair(name, box);
        // A pin to be careful with says so under its box.
        auto *note = new QLabel(&dialog);
        note->setWordWrap(true);
        note->setForegroundRole(QPalette::PlaceholderText);
        const auto describe = [this, note](int gpio) {
            const QString text = Labels::pinNoteText(m_pinNotes.value(gpio), gpio);
            note->setText(text.isEmpty() ? QString() : u"⚠ "_s + text);
            note->setVisible(!text.isEmpty());
        };
        connect(box, &QSpinBox::valueChanged, note, describe);
        describe(box->value());
        form->addRow(QString(), note);
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
            box->setChecked(fromProps.contains(key)                        ? fromProps.value(key).toBool()
                            : kind == u"button" && key == u"activeLow" ? true
                                                                       : p.value(u"default"_s).toBool());
            form->addRow(QString(), box);
            propFields << [key, box] { return qMakePair(key, QJsonValue(box->isChecked())); };
        } else {
            auto *box = new QDoubleSpinBox(&dialog);
            box->setDecimals(type == u"int" ? 0 : 2);
            box->setRange(p.value(u"min"_s).toDouble(-1e9), p.value(u"max"_s).toDouble(1e9));
            box->setValue(fromProps.value(key).toDouble(p.value(u"default"_s).toDouble()));
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
        if (!from.isEmpty()) {
            // The original's bus, at an address no part there has, if the part has one.
            bus->setValue(from.value(u"i2c"_s).toObject().value(u"bus"_s).toInt());
            for (int n = 0; n < address->count(); ++n) {
                if (!usedAddresses.contains(qMakePair(bus->value(), address->itemData(n).toInt()))) {
                    address->setCurrentIndex(n);
                    break;
                }
            }
        }
    }
    auto *labelEdit = new QLineEdit(&dialog);
    labelEdit->setMaxLength(40);
    labelEdit->setPlaceholderText(tr("Optional, e.g. Status LED"));
    labelEdit->setText(from.value(u"label"_s).toString());
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

void CircuitPanel::zoomBy(qreal factor)
{
    const qreal now = m_view->transform().m11();
    const qreal to = std::clamp(now * factor, 0.25, 4.0);
    m_view->setTransformationAnchor(QGraphicsView::AnchorUnderMouse);
    m_view->scale(to / now, to / now);
}

void CircuitPanel::fitView()
{
    const QRectF all = m_scene->itemsBoundingRect().adjusted(-10, -10, 10, 10);
    if (all.isEmpty())
        return;
    m_view->resetTransform();
    m_view->fitInView(all, Qt::KeepAspectRatio);
    // Never bigger than life: small circuits stay at their size.
    if (m_view->transform().m11() > 1.0)
        m_view->resetTransform();
}

qreal CircuitPanel::zoom() const
{
    return m_view->transform().m11();
}

bool CircuitPanel::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == m_view->viewport() && event->type() == QEvent::Wheel) {
        const auto *wheel = static_cast<QWheelEvent *>(event);
        if (!(wheel->modifiers() & Qt::ControlModifier))
            return false;
        if (wheel->angleDelta().y() != 0)
            zoomBy(wheel->angleDelta().y() > 0 ? 1.25 : 0.8);
        return true;
    }
    if (watched != m_view || event->type() != QEvent::KeyPress)
        return QWidget::eventFilter(watched, event);
    const auto *key = static_cast<QKeyEvent *>(event);
    const bool forward = key->key() == Qt::Key_Tab && !(key->modifiers() & Qt::ShiftModifier);
    const bool back = key->key() == Qt::Key_Backtab || (key->key() == Qt::Key_Tab && key->modifiers() & Qt::ShiftModifier);
    if (!forward && !back)
        return QWidget::eventFilter(watched, event);
    QList<QGraphicsItem *> order;
    for (const QJsonValue &v : parts()) {
        if (PartItem *i = m_items.value(v.toObject().value(u"id"_s).toString()))
            order << i;
    }
    const int at = order.indexOf(m_scene->focusItem());
    const int next = at < 0 ? (forward ? 0 : int(order.size()) - 1) : at + (forward ? 1 : -1);
    if (next < 0 || next >= order.size()) {
        m_scene->clearFocus();
        return false; // on to the next control
    }
    order.at(next)->setFocus(forward ? Qt::TabFocusReason : Qt::BacktabFocusReason);
    m_view->ensureVisible(order.at(next));
    return true;
}
