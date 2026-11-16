#include "DisplayPanel.h"

#include "CoreConnection.h"

#include <QLabel>
#include <QLayout>
#include <QPixmap>
#include <QResizeEvent>
#include <QVBoxLayout>

#include <algorithm>

DisplayPanel::DisplayPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_status(new QLabel(this))
    , m_picture(new QLabel(this))
{
    m_status->setWordWrap(true);
    m_status->setText(tr("No picture. Firmware that draws on QEMU's virtual RGB panel "
                         "(esp_lcd_qemu_rgb) shows here while it runs in the simulator."));
    m_status->setToolTip(tr("Display parts such as SSD1306 or ST7789 modules are not simulated; "
                            "only QEMU's own virtual panel is shown."));
    m_picture->setAlignment(Qt::AlignCenter);
    m_picture->setMinimumSize(1, 1);
    m_picture->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Ignored);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_status);
    layout->addWidget(m_picture, 1);

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

void DisplayPanel::rescale()
{
    if (m_image.isNull())
        return;
    // The largest whole multiple that fits, at least 1: no blurred pixels.
    const QSize room = m_picture->size();
    m_scale = std::max(1, std::min(room.width() / m_image.width(), room.height() / m_image.height()));
    const QPixmap pixmap = QPixmap::fromImage(m_image);
    if (room.width() < m_image.width() || room.height() < m_image.height())
        m_picture->setPixmap(pixmap.scaled(room, Qt::KeepAspectRatio, Qt::SmoothTransformation)); // shrink to fit
    else if (m_scale > 1)
        m_picture->setPixmap(pixmap.scaled(m_image.size() * m_scale, Qt::KeepAspectRatio, Qt::FastTransformation));
    else
        m_picture->setPixmap(pixmap);
}
