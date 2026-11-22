// The simulator's virtual RGB panel: the picture the core reports with sim.frame,
// scaled to fit by whole multiples so pixels stay sharp.

#pragma once

#include <QImage>
#include <QWidget>

class CoreConnection;
class QComboBox;
class QLabel;
class QPushButton;
class QScrollArea;

class DisplayPanel : public QWidget {
    Q_OBJECT

public:
    explicit DisplayPanel(CoreConnection *core, QWidget *parent = nullptr);

    QImage image() const { return m_image; }
    QString statusText() const;
    int scale() const { return m_scale; }
    // 0 to fit the dock by whole multiples, else this many screen pixels a pixel.
    void setZoom(int zoom);
    QComboBox *zoomPicker() const { return m_zoom; }
    QPushButton *saveButton() const { return m_save; }
    QPushButton *copyButton() const { return m_copy; }
    // Have the core write the picture to `path` as PNG (frame.save).
    void saveTo(const QString &path);
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

    CoreConnection *m_core;
    QLabel *m_status;
    QLabel *m_picture;
    QScrollArea *m_scroll;
    QComboBox *m_zoom;
    QPushButton *m_save;
    QPushButton *m_copy = nullptr;
    QString m_savingTo; // frame.save is shared with the plot: only this panel's saves
    QImage m_image;
    int m_scale = 1;
    bool m_seenThisSession = false;
};
