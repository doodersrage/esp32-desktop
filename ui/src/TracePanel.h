// The simulation's signal trace: each bridged pin's level over the latest stretch of
// simulated time, drawn as a logic analyzer would, from sim.trace. The core keeps the
// whole trace; Save writes it as VCD for PulseView or GTKWave (sim.traceSave).

#pragma once

#include <QHash>
#include <QJsonArray>
#include <QList>
#include <QWidget>

class CoreConnection;
class QCheckBox;
class QComboBox;
class QLabel;
class QPushButton;

// The waveforms. Presentation only: it draws the edges it is given.
class TraceView : public QWidget {
    Q_OBJECT

public:
    explicit TraceView(QWidget *parent = nullptr);

    struct Edge {
        qint64 t; // ns of simulated time
        bool level;
        bool input;
    };

    void setPins(const QList<int> &gpios);
    void addEdges(const QJsonArray &edges);
    void clear();
    // How much simulated time the width shows, in ns.
    void setSpan(qint64 ns);
    // Stop following new edges (the picture stays put).
    void setHold(bool hold);

    QList<int> pins() const { return m_pins; }
    QList<Edge> edges(int gpio) const { return m_edges.value(gpio); }
    // The latest simulated time seen; the right edge of the picture when following.
    qint64 now() const { return m_now; }

    QSize sizeHint() const override;
    QSize minimumSizeHint() const override;

protected:
    void paintEvent(QPaintEvent *event) override;

private:
    QString timeLabel(qint64 ns) const;

    QList<int> m_pins;
    QHash<int, QList<Edge>> m_edges;
    qint64 m_now = 0;
    qint64 m_end = 0; // right edge of the picture
    qint64 m_span = 10'000'000;
    bool m_hold = false;
};

class TracePanel : public QWidget {
    Q_OBJECT

public:
    explicit TracePanel(CoreConnection *core, QWidget *parent = nullptr);

    TraceView *view() const { return m_view; }
    QString statusText() const;
    QPushButton *saveButton() const { return m_save; }
    QComboBox *spanBox() const { return m_span; }

    // Ask the core to write the trace to `path` (the Save button asks for one).
    void saveTo(const QString &path);

private:
    void started();
    void stopped();
    void save();

    CoreConnection *m_core;
    QLabel *m_status;
    QComboBox *m_span;
    QCheckBox *m_hold;
    QPushButton *m_save;
    TraceView *m_view;
};
