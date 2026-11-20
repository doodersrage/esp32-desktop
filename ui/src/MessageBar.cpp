#include "MessageBar.h"

#include <QHBoxLayout>
#include <QIcon>
#include <QLabel>
#include <QStyle>
#include <QToolButton>

using namespace Qt::StringLiterals;

MessageBar::MessageBar(QWidget *parent)
    : QFrame(parent)
    , m_icon(new QLabel(this))
    , m_text(new QLabel(this))
    , m_close(new QToolButton(this))
{
    setFrameShape(QFrame::StyledPanel);
    setAutoFillBackground(true);
    setBackgroundRole(QPalette::AlternateBase);
    const int size = style()->pixelMetric(QStyle::PM_SmallIconSize, nullptr, this);
    m_icon->setPixmap(QIcon::fromTheme(u"dialog-error"_s, style()->standardIcon(QStyle::SP_MessageBoxCritical))
                          .pixmap(size, size));
    m_text->setWordWrap(true);
    m_text->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::LinksAccessibleByMouse);
    m_text->setOpenExternalLinks(true);
    m_close->setIcon(QIcon::fromTheme(u"window-close"_s, style()->standardIcon(QStyle::SP_TitleBarCloseButton)));
    m_close->setAutoRaise(true);
    m_close->setToolTip(tr("Close this message"));
    connect(m_close, &QToolButton::clicked, this, &QWidget::hide);

    auto *layout = new QHBoxLayout(this);
    layout->addWidget(m_icon, 0, Qt::AlignTop);
    layout->addWidget(m_text, 1);
    layout->addWidget(m_close, 0, Qt::AlignTop);
    hide();
}

void MessageBar::showError(const QString &text)
{
    m_text->setTextFormat(Qt::PlainText);
    m_text->setText(text);
    show();
}

void MessageBar::showExplanation(const QString &text, const QString &url)
{
    QString html = text.toHtmlEscaped();
    if (url.startsWith(u"https://"_s))
        html += u" <a href=\"%1\">%2</a>"_s.arg(url.toHtmlEscaped(), tr("Read more on esp32bench.dev"));
    m_text->setTextFormat(Qt::RichText);
    m_text->setText(html);
    m_plain = text;
    show();
}

QString MessageBar::text() const
{
    if (isHidden())
        return {};
    return m_text->textFormat() == Qt::RichText ? m_plain : m_text->text();
}
