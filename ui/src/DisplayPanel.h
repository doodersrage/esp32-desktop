// The simulator's virtual RGB panel: the picture the core reports with sim.frame,
// scaled to fit by whole multiples so pixels stay sharp.

#pragma once

#include <QImage>
#include <QWidget>

class CoreConnection;
class QLabel;

class DisplayPanel : public QWidget {
    Q_OBJECT

public:
    explicit DisplayPanel(CoreConnection *core, QWidget *parent = nullptr);

    QImage image() const { return m_image; }
    QString statusText() const;
    int scale() const { return m_scale; }
    // Room for the picture at its own size (or a typical panel before the first one).
    QSize sizeHint() const override;

signals:
    // The first picture of a simulator session: time to show the panel.
    void firstFrame();

protected:
    void resizeEvent(QResizeEvent *event) override;

private:
    void showFrame(const QImage &image);
    void rescale();

    QLabel *m_status;
    QLabel *m_picture;
    QImage m_image;
    int m_scale = 1;
    bool m_seenThisSession = false;
};
