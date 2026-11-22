// Numbers the firmware prints, drawn as lines over time, like the Arduino Serial
// Plotter. The core finds them (serial.data's and sim.output's plot rows); this
// renders the latest rows of whichever is running.

#pragma once

#include <QImage>

#include <QHash>
#include <QRectF>
#include <QSet>
#include <utility>
#include <QJsonArray>
#include <QWidget>

class CoreConnection;
class QCheckBox;
class QLabel;
class QPushButton;

class PlotCanvas : public QWidget {
    Q_OBJECT

public:
    explicit PlotCanvas(QWidget *parent = nullptr);
    // Add rows (arrays of {name, value}); keeps the latest Kept of each series.
    void addRows(const QJsonArray &rows);
    void clear();
    QStringList seriesNames() const { return m_order; }
    QList<double> values(const QString &name) const { return m_series.value(name); }
    // A line hidden by a click on its legend entry (not drawn, not in the scale).
    void setHidden(const QString &name, bool hidden);
    bool isHidden(const QString &name) const { return m_hidden.contains(name); }
    // Where each legend entry is, from the last paint (for clicks, and tests).
    QList<std::pair<QRectF, QString>> legend() const { return m_legend; }

    static constexpr int Kept = 500;

protected:
    void paintEvent(QPaintEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void leaveEvent(QEvent *event) override;

private:
    QSet<QString> m_hidden;
    QList<std::pair<QRectF, QString>> m_legend;
    qreal m_hoverX = -1;
    QHash<QString, QList<double>> m_series;
    QStringList m_order;
    // Rows seen, so series that start later line up with the others.
    qint64 m_rows = 0;
    QHash<QString, qint64> m_firstRow;
};

class PlotPanel : public QWidget {
    Q_OBJECT

public:
    explicit PlotPanel(CoreConnection *core, QWidget *parent = nullptr);
    PlotCanvas *canvas() const { return m_canvas; }
    // The rows received since the run started (at most KeptRows), as the core sent them.
    QJsonArray rows() const;
    // Have the core write them to `path` as CSV (plot.save); Save CSV… asks for it.
    void saveTo(const QString &path);
    QPushButton *saveButton() const { return m_save; }
    QPushButton *savePictureButton() const { return m_savePicture; }
    // The plot as drawn, at most 4096 pixels a side (frame.save's limit).
    QImage picture() const;
    // Have the core write it to `path` as PNG (frame.save); Save picture… asks for it.
    void savePictureTo(const QString &path);
    static constexpr int KeptRows = 100000;

signals:
    // The first rows of a run arrived: worth showing the dock.
    void firstRows();

private:
    CoreConnection *m_core;
    PlotCanvas *m_canvas;
    QLabel *m_status;
    QCheckBox *m_pause;
    QPushButton *m_clear;
    QPushButton *m_save;
    QPushButton *m_savePicture = nullptr;
    QString m_savingTo;
    QList<QJsonValue> m_rows; // QList drops its first item cheaply
    bool m_seen = false;

    void clearRows();
};
