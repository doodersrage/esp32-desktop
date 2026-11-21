#include "PlotPanel.h"
#include <QStandardPaths>
#include <QFileDialog>
#include <QDir>
#include <QDateTime>

#include "CoreConnection.h"

#include <QCheckBox>
#include <QHBoxLayout>
#include <QIcon>
#include <QJsonObject>
#include <QLabel>
#include <QPainter>
#include <QPainterPath>
#include <QPushButton>
#include <QVBoxLayout>

#include <algorithm>
#include <cmath>

using namespace Qt::StringLiterals;

PlotCanvas::PlotCanvas(QWidget *parent)
    : QWidget(parent)
{
    setMinimumHeight(120);
    setAutoFillBackground(true);
    setBackgroundRole(QPalette::Base);
}

void PlotCanvas::addRows(const QJsonArray &rows)
{
    for (const QJsonValue &r : rows) {
        for (const QJsonValue &v : r.toArray()) {
            const QJsonObject o = v.toObject();
            const QString name = o.value(u"name"_s).toString();
            if (!m_series.contains(name)) {
                if (m_order.size() >= 16)
                    continue; // as many lines as anyone can tell apart
                m_order << name;
                m_firstRow.insert(name, m_rows);
            }
            QList<double> &s = m_series[name];
            s.append(o.value(u"value"_s).toDouble());
            if (s.size() > Kept)
                s.removeFirst();
        }
        ++m_rows;
    }
    update();
}

void PlotCanvas::clear()
{
    m_series.clear();
    m_order.clear();
    m_firstRow.clear();
    m_rows = 0;
    update();
}

void PlotCanvas::paintEvent(QPaintEvent *)
{
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);
    const QPalette pal = palette();
    const QFontMetrics fm(font());
    const int legendH = fm.height() + 6;
    const int left = fm.horizontalAdvance(u"-00000.0"_s) + 8;
    const QRectF area(left, legendH + 4, width() - left - 8, height() - legendH - 12);
    p.setPen(pal.color(QPalette::Mid));
    p.drawRect(area);
    if (m_order.isEmpty()) {
        p.setPen(pal.color(QPalette::PlaceholderText));
        p.drawText(area, Qt::AlignCenter | Qt::TextWordWrap,
                   tr("Lines of numbers the firmware prints are drawn here, such as \"21.5 45\" or "
                      "\"temp:21.5 humidity:45\"."));
        return;
    }
    double lo = INFINITY, hi = -INFINITY;
    for (const QString &n : m_order) {
        for (double v : m_series.value(n)) {
            lo = std::min(lo, v);
            hi = std::max(hi, v);
        }
    }
    if (hi - lo < 1e-9) {
        lo -= 1;
        hi += 1;
    }
    const double pad = (hi - lo) * 0.05;
    lo -= pad;
    hi += pad;
    // The scale: top, middle and bottom values.
    p.setPen(pal.color(QPalette::Text));
    for (int i = 0; i <= 2; ++i) {
        const double v = hi - (hi - lo) * i / 2;
        const qreal y = area.top() + area.height() * i / 2;
        p.drawText(QRectF(0, y - fm.height() / 2.0, left - 4, fm.height()), Qt::AlignRight | Qt::AlignVCenter,
                   QString::number(v, 'g', 5));
        p.setPen(QPen(pal.color(QPalette::Midlight), 1, Qt::DotLine));
        p.drawLine(QPointF(area.left(), y), QPointF(area.right(), y));
        p.setPen(pal.color(QPalette::Text));
    }
    // One line per series over the latest Kept rows; distinct hues, which read on light
    // and dark backgrounds (the plot is a drawing, like the circuit canvas).
    const qint64 firstShown = std::max<qint64>(0, m_rows - Kept);
    qreal legendX = left;
    for (int i = 0; i < m_order.size(); ++i) {
        const QString &name = m_order.at(i);
        const QColor colour = QColor::fromHsv((i * 137) % 360, 200, 210);
        const QList<double> s = m_series.value(name);
        const qint64 start = std::max(m_firstRow.value(name), m_rows - s.size());
        QPainterPath path;
        for (int k = 0; k < s.size(); ++k) {
            const qint64 row = start + k;
            if (row < firstShown)
                continue;
            const QPointF pt(area.left() + area.width() * (row - firstShown) / std::max<qint64>(1, Kept - 1),
                             area.bottom() - area.height() * (s.at(k) - lo) / (hi - lo));
            if (path.elementCount() == 0)
                path.moveTo(pt);
            else
                path.lineTo(pt);
        }
        p.setPen(QPen(colour, 2));
        p.drawPath(path);
        // The legend, with the latest value.
        const QString label = tr("%1: %2").arg(name).arg(s.isEmpty() ? 0.0 : s.last(), 0, 'g', 6);
        p.fillRect(QRectF(legendX, 4 + fm.height() / 2.0 - 2, 12, 4), colour);
        p.setPen(pal.color(QPalette::Text));
        p.drawText(QPointF(legendX + 16, 4 + fm.ascent()), label);
        legendX += 16 + fm.horizontalAdvance(label) + 16;
    }
}

PlotPanel::PlotPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_canvas(new PlotCanvas(this))
    , m_status(new QLabel(this))
    , m_pause(new QCheckBox(tr("Pause"), this))
    , m_clear(new QPushButton(QIcon::fromTheme(u"edit-clear"_s), tr("Clear"), this))
    , m_save(new QPushButton(QIcon::fromTheme(u"document-save"_s), tr("Save CSV…"), this))
{
    m_save->setToolTip(tr("Save the numbers received so far as a CSV file, for a spreadsheet"));
    m_save->setEnabled(false);
    connect(m_save, &QPushButton::clicked, this, [this] {
        const QString dir = QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation);
        const QString name = u"esp32bench-plot-%1.csv"_s.arg(QDateTime::currentDateTime().toString(u"yyyyMMdd-HHmm"_s));
        const QString path = QFileDialog::getSaveFileName(this, tr("Save the plot"), QDir(dir).filePath(name),
                                                          tr("CSV files (*.csv)"));
        if (!path.isEmpty())
            saveTo(path);
    });
    connect(m_core, &CoreConnection::plotSaved, this, [this](const QString &path, qint64 rows) {
        m_status->setText(tr("Saved %1 rows to %2.").arg(rows).arg(path));
    });
    m_status->setText(tr("Numbers from the simulator or the serial monitor, whichever is running."));
    m_status->setWordWrap(true);
    m_canvas->setAccessibleName(tr("Plot of the numbers received"));
    m_pause->setToolTip(tr("Stop adding new values, to look at what is shown"));
    connect(m_clear, &QPushButton::clicked, this, &PlotPanel::clearRows);
    const auto add = [this](const QJsonArray &rows) {
        if (rows.isEmpty() || m_pause->isChecked())
            return;
        if (!m_seen) {
            m_seen = true;
            emit firstRows();
        }
        m_canvas->addRows(rows);
        for (const QJsonValue &r : rows)
            m_rows.append(r);
        while (m_rows.size() > KeptRows)
            m_rows.removeFirst();
        m_save->setEnabled(true);
    };
    connect(m_core, &CoreConnection::plotRows, this, add);
    // A new run starts a new plot.
    connect(m_core, &CoreConnection::simStarted, this, [this] {
        clearRows();
        m_seen = false;
    });
    connect(m_core, &CoreConnection::serialOpened, this, [this] {
        clearRows();
        m_seen = false;
    });

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_pause);
    top->addWidget(m_clear);
    top->addWidget(m_save);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_canvas, 1);
}

void PlotPanel::clearRows()
{
    m_canvas->clear();
    m_rows = {};
    m_save->setEnabled(false);
}

QJsonArray PlotPanel::rows() const
{
    QJsonArray out;
    for (const QJsonValue &r : m_rows)
        out.append(r);
    return out;
}

void PlotPanel::saveTo(const QString &path)
{
    m_core->savePlot(path, rows());
}
