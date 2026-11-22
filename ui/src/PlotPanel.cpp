#include "PlotPanel.h"

#include <QMouseEvent>
#include <cmath>
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
    setMouseTracking(true); // values under the mouse
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
    m_hidden.clear();
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
    // The scale fits the lines shown; hidden ones (a click on their legend) do not count.
    double lo = INFINITY, hi = -INFINITY;
    for (const QString &n : m_order) {
        if (m_hidden.contains(n))
            continue;
        for (double v : m_series.value(n)) {
            lo = std::min(lo, v);
            hi = std::max(hi, v);
        }
    }
    if (!std::isfinite(lo)) {
        lo = 0;
        hi = 1;
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
    m_legend.clear();
    // The row under the mouse, for reading values off the lines.
    qint64 hoverRow = -1;
    if (m_hoverX >= area.left() && m_hoverX <= area.right())
        hoverRow = firstShown + qRound((m_hoverX - area.left()) / area.width() * std::max<qint64>(1, Kept - 1));
    QStringList readings;
    for (int i = 0; i < m_order.size(); ++i) {
        const QString &name = m_order.at(i);
        const bool hidden = m_hidden.contains(name);
        const QColor colour = QColor::fromHsv((i * 137) % 360, hidden ? 40 : 200, 210);
        const QList<double> s = m_series.value(name);
        const qint64 start = std::max(m_firstRow.value(name), m_rows - s.size());
        if (hoverRow >= start && hoverRow < start + s.size() && !hidden)
            readings << tr("%1: %2").arg(name).arg(s.at(hoverRow - start), 0, 'g', 6);
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
        if (!hidden) {
            p.setPen(QPen(colour, 2));
            p.drawPath(path);
        }
        // The legend, with the latest value; a click on it hides or shows the line.
        const QString label = tr("%1: %2").arg(name).arg(s.isEmpty() ? 0.0 : s.last(), 0, 'g', 6);
        p.fillRect(QRectF(legendX, 4 + fm.height() / 2.0 - 2, 12, 4), colour);
        p.setPen(pal.color(hidden ? QPalette::PlaceholderText : QPalette::Text));
        p.drawText(QPointF(legendX + 16, 4 + fm.ascent()), label);
        const qreal w = 16 + fm.horizontalAdvance(label);
        m_legend.append({QRectF(legendX, 2, w, legendH), name});
        legendX += w + 16;
    }
    if (hoverRow >= 0 && !readings.isEmpty()) {
        p.setPen(QPen(pal.color(QPalette::Highlight), 1, Qt::DashLine));
        p.drawLine(QPointF(m_hoverX, area.top()), QPointF(m_hoverX, area.bottom()));
        const QString text = readings.join(u"   "_s);
        const QRectF box(std::min(m_hoverX + 6, area.right() - fm.horizontalAdvance(text) - 8), area.top() + 4,
                         fm.horizontalAdvance(text) + 8, fm.height() + 4);
        p.fillRect(box, pal.color(QPalette::ToolTipBase));
        p.setPen(pal.color(QPalette::ToolTipText));
        p.drawText(box, Qt::AlignCenter, text);
    }
}

void PlotCanvas::setHidden(const QString &name, bool hidden)
{
    if (hidden)
        m_hidden.insert(name);
    else
        m_hidden.remove(name);
    update();
}

void PlotCanvas::mousePressEvent(QMouseEvent *event)
{
    for (const auto &[rect, name] : std::as_const(m_legend)) {
        if (rect.contains(event->position())) {
            setHidden(name, !m_hidden.contains(name));
            return;
        }
    }
    QWidget::mousePressEvent(event);
}

void PlotCanvas::mouseMoveEvent(QMouseEvent *event)
{
    m_hoverX = event->position().x();
    update();
}

void PlotCanvas::leaveEvent(QEvent *event)
{
    m_hoverX = -1;
    update();
    QWidget::leaveEvent(event);
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
    m_savePicture = new QPushButton(QIcon::fromTheme(u"document-save"_s), tr("Save picture…"), this);
    m_savePicture->setToolTip(tr("Save the plot as drawn as a PNG picture, to share it"));
    m_savePicture->setEnabled(false);
    connect(m_savePicture, &QPushButton::clicked, this, [this] {
        const QString dir = QStandardPaths::writableLocation(QStandardPaths::PicturesLocation);
        const QString name = u"esp32bench-plot-%1.png"_s.arg(QDateTime::currentDateTime().toString(u"yyyyMMdd-HHmm"_s));
        const QString path = QFileDialog::getSaveFileName(this, tr("Save the plot as a picture"),
                                                          QDir(dir).filePath(name), tr("PNG pictures (*.png)"));
        if (!path.isEmpty())
            savePictureTo(path);
    });
    connect(m_core, &CoreConnection::frameSaved, this, [this](const QString &path) {
        if (path == std::exchange(m_savingTo, {}))
            m_status->setText(tr("Saved the picture to %1.").arg(path));
    });
    connect(m_core, &CoreConnection::plotSaved, this, [this](const QString &path, qint64 rows) {
        m_status->setText(tr("Saved %1 rows to %2.").arg(rows).arg(path));
    });
    m_status->setText(tr("Numbers from the simulator or the serial monitor, whichever is running. Click a name "
                         "in the legend to hide or show its line; point at the plot to read its values."));
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
        m_savePicture->setEnabled(true);
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
    top->addWidget(m_savePicture);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_canvas, 1);
}

void PlotPanel::clearRows()
{
    m_canvas->clear();
    m_rows = {};
    m_save->setEnabled(false);
    m_savePicture->setEnabled(false);
}

QImage PlotPanel::picture() const
{
    QImage image = m_canvas->grab().toImage();
    if (image.width() > 4096 || image.height() > 4096)
        image = image.scaled(4096, 4096, Qt::KeepAspectRatio, Qt::SmoothTransformation);
    return image;
}

void PlotPanel::savePictureTo(const QString &path)
{
    m_savingTo = path;
    m_core->saveFrame(path, picture());
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
