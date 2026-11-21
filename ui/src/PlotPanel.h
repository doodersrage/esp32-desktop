// Numbers the firmware prints, drawn as lines over time, like the Arduino Serial
// Plotter. The core finds them (serial.data's and sim.output's plot rows); this
// renders the latest rows of whichever is running.

#pragma once

#include <QHash>
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

    static constexpr int Kept = 500;

protected:
    void paintEvent(QPaintEvent *event) override;

private:
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
    QList<QJsonValue> m_rows; // QList drops its first item cheaply
    bool m_seen = false;

    void clearRows();
};
