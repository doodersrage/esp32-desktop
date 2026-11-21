#include "DisplayPanel.h"

#include "CoreConnection.h"

#include <QComboBox>
#include <QDateTime>
#include <QDir>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QIcon>
#include <QPushButton>
#include <QScrollArea>
#include <QStandardPaths>
#include <QLabel>
#include <QLayout>
#include <QPixmap>
#include <QResizeEvent>
#include <QVBoxLayout>

#include <algorithm>

using namespace Qt::StringLiterals;

DisplayPanel::DisplayPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(this))
    , m_picture(new QLabel)
    , m_scroll(new QScrollArea(this))
    , m_zoom(new QComboBox(this))
    , m_save(new QPushButton(QIcon::fromTheme(u"document-save"_s), tr("Save picture…"), this))
{
    m_zoom->addItem(tr("Fit"), 0);
    for (int z : {1, 2, 3, 4, 6, 8})
        m_zoom->addItem(tr("%1×").arg(z), z);
    m_zoom->setAccessibleName(tr("Zoom"));
    m_zoom->setToolTip(tr("Fit the dock, or a fixed size: each panel pixel this many screen pixels"));
    connect(m_zoom, &QComboBox::currentIndexChanged, this, [this] { rescale(); });
    m_save->setToolTip(tr("Save the picture shown as a PNG file"));
    m_save->setEnabled(false);
    connect(m_save, &QPushButton::clicked, this, [this] {
        const QString dir = QStandardPaths::writableLocation(QStandardPaths::PicturesLocation);
        const QString path = QFileDialog::getSaveFileName(
            this, tr("Save the picture"),
            QDir(dir).filePath(u"esp32bench-panel-%1.png"_s.arg(QDateTime::currentDateTime().toString(u"yyyyMMdd-HHmmss"_s))),
            tr("PNG pictures (*.png)"));
        if (!path.isEmpty())
            saveTo(path);
    });
    connect(core, &CoreConnection::frameSaved, this,
            [this](const QString &path) { m_status->setText(tr("Saved the picture to %1.").arg(path)); });
    m_scroll->setWidget(m_picture);
    m_scroll->setWidgetResizable(true);
    m_scroll->setFrameShape(QFrame::NoFrame);
    m_scroll->setAlignment(Qt::AlignCenter);
    m_status->setWordWrap(true);
    m_status->setText(tr("No picture. Firmware that draws on QEMU's virtual RGB panel "
                         "(esp_lcd_qemu_rgb) shows here while it runs in the simulator."));
    m_status->setToolTip(tr("Only QEMU's own virtual panel shows here. An SSD1306 OLED placed in the Circuit "
                            "dock draws on the canvas there; ST7789 modules are not simulated."));
    m_picture->setAlignment(Qt::AlignCenter);
    m_picture->setMinimumSize(1, 1);
    m_picture->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Ignored);

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_zoom);
    top->addWidget(m_save);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_scroll, 1);

    connect(core, &CoreConnection::simFrame, this, &DisplayPanel::showFrame);
    connect(core, &CoreConnection::simStarted, this, [this] {
        m_seenThisSession = false;
        m_image = QImage();
        m_picture->clear();
        m_status->setText(tr("Waiting for the firmware to draw."));
    });
    connect(core, &CoreConnection::simStopped, this, [this] {
        m_status->setText(m_image.isNull() ? tr("The simulator has stopped. This firmware did not draw on the virtual panel.")
                                           : tr("The simulator has stopped; this is its last picture."));
    });
}

QSize DisplayPanel::sizeHint() const
{
    const QSize picture = m_image.isNull() ? QSize(320, 240) : m_image.size();
    const QMargins m = layout()->contentsMargins();
    return picture + QSize(m.left() + m.right(), m.top() + m.bottom() + m_status->sizeHint().height()
                                                     + layout()->spacing());
}

QString DisplayPanel::statusText() const
{
    return m_status->text();
}

void DisplayPanel::showFrame(const QImage &image)
{
    m_image = image;
    m_status->setText(tr("%1 × %2 pixels").arg(image.width()).arg(image.height()));
    rescale();
    if (!m_seenThisSession) {
        m_seenThisSession = true;
        emit firstFrame();
    }
}

void DisplayPanel::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    rescale();
}

void DisplayPanel::setZoom(int zoom)
{
    const int i = m_zoom->findData(zoom);
    m_zoom->setCurrentIndex(i < 0 ? 0 : i);
}

void DisplayPanel::saveTo(const QString &path)
{
    if (!m_image.isNull())
        m_core->saveFrame(path, m_image);
}

void DisplayPanel::rescale()
{
    if (m_image.isNull())
        return;
    m_save->setEnabled(true);
    const int fixed = m_zoom->currentData().toInt();
    if (fixed > 0) {
        // A fixed size; the dock scrolls when it is bigger.
        m_scale = fixed;
        m_scroll->setWidgetResizable(false);
        const QPixmap pixmap = QPixmap::fromImage(m_image).scaled(m_image.size() * fixed, Qt::KeepAspectRatio,
                                                                  Qt::FastTransformation);
        m_picture->setPixmap(pixmap);
        m_picture->resize(pixmap.size());
        return;
    }
    m_scroll->setWidgetResizable(true);
    // The largest whole multiple that fits, at least 1: no blurred pixels.
    const QSize room = m_scroll->viewport()->size();
    m_scale = std::max(1, std::min(room.width() / m_image.width(), room.height() / m_image.height()));
    const QPixmap pixmap = QPixmap::fromImage(m_image);
    if (room.width() < m_image.width() || room.height() < m_image.height())
        m_picture->setPixmap(pixmap.scaled(room, Qt::KeepAspectRatio, Qt::SmoothTransformation)); // shrink to fit
    else if (m_scale > 1)
        m_picture->setPixmap(pixmap.scaled(m_image.size() * m_scale, Qt::KeepAspectRatio, Qt::FastTransformation));
    else
        m_picture->setPixmap(pixmap);
}
