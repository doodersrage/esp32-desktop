#include "TracePanel.h"

#include "CoreConnection.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QJsonObject>
#include <QLabel>
#include <QPainter>
#include <QPushButton>
#include <QStandardPaths>
#include <QVBoxLayout>

#include <algorithm>
#include <optional>

using namespace Qt::StringLiterals;

namespace {
// Edges kept per pin for drawing; the core keeps the full trace for saving.
constexpr qsizetype MaxEdgesPerPin = 50'000;
constexpr int RowHeight = 28;
constexpr int LabelWidth = 64;
constexpr int AxisHeight = 22;
} // namespace

TraceView::TraceView(QWidget *parent)
    : QWidget(parent)
{
    setAutoFillBackground(true);
    setBackgroundRole(QPalette::Base);
}

void TraceView::setPins(const QList<int> &gpios)
{
    m_pins = gpios;
    std::sort(m_pins.begin(), m_pins.end());
    updateGeometry();
    update();
}

void TraceView::clear()
{
    m_edges.clear();
    m_now = m_end = 0;
    update();
}

void TraceView::addEdges(const QJsonArray &edges)
{
    for (const QJsonValue &v : edges) {
        const QJsonObject o = v.toObject();
        const int gpio = o.value(u"gpio"_s).toInt(-1);
        if (!m_pins.contains(gpio))
            continue;
        QList<Edge> &list = m_edges[gpio];
        list.append({o.value(u"t"_s).toInteger(), o.value(u"level"_s).toBool(), o.value(u"input"_s).toBool()});
        if (list.size() > MaxEdgesPerPin)
            list.remove(0, list.size() - MaxEdgesPerPin);
        m_now = std::max(m_now, list.last().t);
    }
    if (!m_hold) {
        m_end = m_now;
        update();
    }
}

void TraceView::setSpan(qint64 ns)
{
    m_span = std::max<qint64>(ns, 1000);
    update();
}

void TraceView::setHold(bool hold)
{
    m_hold = hold;
    if (!hold) {
        m_end = m_now;
        update();
    }
}

QSize TraceView::sizeHint() const
{
    return {600, AxisHeight + RowHeight * int(std::max<qsizetype>(m_pins.size(), 2)) + 4};
}

QSize TraceView::minimumSizeHint() const
{
    return {LabelWidth + 100, AxisHeight + RowHeight + 4};
}

QString TraceView::timeLabel(qint64 ns) const
{
    // Relative to the right edge, in the unit that suits the span.
    const QLocale loc;
    if (m_span >= 2'000'000'000)
        return tr("%1 s").arg(loc.toString(double(ns) / 1e9, 'g', 3));
    if (m_span >= 2'000'000)
        return tr("%1 ms").arg(loc.toString(double(ns) / 1e6, 'g', 3));
    return tr("%1 µs").arg(loc.toString(double(ns) / 1e3, 'g', 3));
}

void TraceView::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    const QPalette &pal = palette();
    const int plotLeft = LabelWidth;
    const int plotWidth = std::max(1, width() - plotLeft - 8);
    const qint64 start = m_end - m_span;
    auto xFor = [&](qint64 t) {
        return plotLeft + int(double(t - start) * plotWidth / double(m_span));
    };

    // Time axis: five divisions back from the right edge.
    p.setPen(pal.color(QPalette::Mid));
    for (int i = 0; i <= 5; ++i) {
        const int x = plotLeft + plotWidth * i / 5;
        p.drawLine(x, AxisHeight - 4, x, height());
    }
    p.setPen(pal.color(QPalette::PlaceholderText));
    for (int i = 0; i < 5; ++i) {
        const int x = plotLeft + plotWidth * i / 5;
        p.drawText(QRect(x + 3, 0, plotWidth / 5, AxisHeight - 4), Qt::AlignLeft | Qt::AlignVCenter,
                   timeLabel(-(m_span * (5 - i) / 5)));
    }

    QPen line(pal.color(QPalette::Text), 1.5);
    QPen inputLine(pal.color(QPalette::Highlight), 1.5);
    QPen unknown(pal.color(QPalette::Mid), 1, Qt::DashLine);
    for (int row = 0; row < m_pins.size(); ++row) {
        const int gpio = m_pins[row];
        const int top = AxisHeight + row * RowHeight + 5;
        const int high = top, low = top + RowHeight - 10;
        p.setPen(pal.color(QPalette::Text));
        p.drawText(QRect(4, top, LabelWidth - 8, low - top), Qt::AlignLeft | Qt::AlignVCenter,
                   tr("GPIO %1").arg(gpio));

        const QList<Edge> list = m_edges.value(gpio);
        // The level at the left edge: the last edge before it, if there is one.
        auto first = std::lower_bound(list.cbegin(), list.cend(), start,
                                      [](const Edge &e, qint64 t) { return e.t < t; });
        std::optional<Edge> cur;
        if (first != list.cbegin())
            cur = *(first - 1);
        int x = plotLeft;
        auto drawTo = [&](int xEnd) {
            if (!cur) {
                p.setPen(unknown);
                p.drawLine(x, (high + low) / 2, xEnd, (high + low) / 2);
            } else {
                p.setPen(cur->input ? inputLine : line);
                const int y = cur->level ? high : low;
                p.drawLine(x, y, xEnd, y);
            }
        };
        for (auto it = first; it != list.cend() && it->t <= m_end; ++it) {
            const int xe = std::clamp(xFor(it->t), plotLeft, plotLeft + plotWidth);
            drawTo(xe);
            p.setPen(it->input ? inputLine : line);
            if (cur && cur->level != it->level)
                p.drawLine(xe, high, xe, low);
            cur = *it;
            x = xe;
        }
        // Up to the latest time seen; nothing is known beyond it.
        drawTo(std::clamp(xFor(std::min(m_end, m_now)), plotLeft, plotLeft + plotWidth));
    }
}

TracePanel::TracePanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(this))
    , m_span(new QComboBox(this))
    , m_hold(new QCheckBox(tr("Hold"), this))
    , m_save(new QPushButton(QIcon::fromTheme(u"document-save"_s), tr("Save VCD…"), this))
    , m_view(new TraceView(this))
{
    m_status->setWordWrap(true);
    m_status->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_span->addItem(tr("100 µs"), 100'000LL);
    m_span->addItem(tr("1 ms"), 1'000'000LL);
    m_span->addItem(tr("10 ms"), 10'000'000LL);
    m_span->addItem(tr("100 ms"), 100'000'000LL);
    m_span->addItem(tr("1 s"), 1'000'000'000LL);
    m_span->addItem(tr("10 s"), 10'000'000'000LL);
    m_span->setCurrentIndex(3);
    m_span->setToolTip(tr("How much simulated time the trace shows"));
    m_view->setSpan(m_span->currentData().toLongLong());
    m_hold->setToolTip(tr("Stop following new changes, to look at the picture"));
    m_save->setToolTip(tr("Write every recorded pin change to a VCD file, for PulseView or GTKWave"));
    m_save->setEnabled(false);

    auto *bar = new QHBoxLayout;
    bar->addWidget(m_status, 1);
    bar->addWidget(m_span);
    bar->addWidget(m_hold);
    bar->addWidget(m_save);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(bar);
    layout->addWidget(m_view, 1);

    connect(m_span, &QComboBox::currentIndexChanged, this,
            [this] { m_view->setSpan(m_span->currentData().toLongLong()); });
    connect(m_hold, &QCheckBox::toggled, m_view, &TraceView::setHold);
    connect(m_save, &QPushButton::clicked, this, &TracePanel::save);
    connect(core, &CoreConnection::simStarted, this, &TracePanel::started);
    connect(core, &CoreConnection::simStopped, this, &TracePanel::stopped);
    connect(core, &CoreConnection::simTrace, this, [this](const QJsonArray &edges, qint64) {
        m_view->addEdges(edges);
    });
    connect(core, &CoreConnection::traceSaved, this, [this](const QString &path, qint64 edges, qint64 dropped) {
        m_status->setText(dropped > 0 ? tr("Saved %1 pin changes to %2 (the earliest %3 were no longer kept).")
                                            .arg(edges)
                                            .arg(path)
                                            .arg(dropped)
                                      : tr("Saved %1 pin changes to %2.").arg(edges).arg(path));
    });
    connect(core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method == u"sim.traceSave")
            m_status->setText(tr("Could not save the trace: %1").arg(message));
    });
    m_status->setText(tr("No simulation. Pin changes of a simulation with pin access are traced here."));
}

QString TracePanel::statusText() const
{
    return m_status->text();
}

void TracePanel::started()
{
    m_view->clear();
    m_view->setPins(m_core->simPins());
    m_save->setEnabled(m_core->canSaveTrace());
    m_status->setText(m_core->simPins().isEmpty()
                          ? tr("No pin access in this simulation, so there is nothing to trace.")
                          : tr("Tracing in simulated time."));
}

void TracePanel::stopped()
{
    m_save->setEnabled(m_core->canSaveTrace());
    if (m_core->canSaveTrace())
        m_status->setText(tr("The simulation has stopped; its trace can still be saved."));
}

void TracePanel::save()
{
    const QString dir = QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation);
    const QString path = QFileDialog::getSaveFileName(this, tr("Save trace"), dir + u"/trace.vcd"_s,
                                                      tr("Value change dumps (*.vcd)"));
    if (!path.isEmpty())
        saveTo(path);
}

void TracePanel::saveTo(const QString &path)
{
    m_status->setText(tr("Saving…"));
    m_core->saveTrace(path);
}
