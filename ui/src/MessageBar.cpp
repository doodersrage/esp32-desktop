#include "MessageBar.h"

#include <QHBoxLayout>
#include <QIcon>
#include <QClipboard>
#include <QGuiApplication>
#include <QLabel>
#include <QPushButton>
#include <QStyle>
#include <QToolButton>

#include <utility>

using namespace Qt::StringLiterals;

MessageBar::MessageBar(QWidget *parent)
    : QFrame(parent)
    , m_icon(new QLabel(this))
    , m_text(new QLabel(this))
    , m_close(new QToolButton(this))
    , m_action(new QPushButton(this))
    , m_copy(new QPushButton(QIcon::fromTheme(u"edit-copy"_s), tr("Copy details"), this))
{
    m_copy->setToolTip(tr("Copy the message and the app's versions, to paste into a question or a problem report"));
    m_copy->hide();
    connect(m_copy, &QPushButton::clicked, this,
            [this] { QGuiApplication::clipboard()->setText(detailsText()); });
    m_action->setText(tr("Undo")); // until a message names its own
    m_action->hide();
    connect(m_action, &QPushButton::clicked, this, [this] {
        hide();
        if (auto undo = std::exchange(m_undo, {}))
            undo();
    });
    setFrameShape(QFrame::StyledPanel);
    setAutoFillBackground(true);
    setBackgroundRole(QPalette::AlternateBase);
    setIcon(false);
    m_text->setWordWrap(true);
    m_text->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::LinksAccessibleByMouse);
    m_text->setOpenExternalLinks(true);
    m_close->setIcon(QIcon::fromTheme(u"window-close"_s, style()->standardIcon(QStyle::SP_TitleBarCloseButton)));
    m_close->setAutoRaise(true);
    m_close->setAccessibleName(tr("Close this message"));
    m_close->setToolTip(tr("Close this message"));
    connect(m_close, &QToolButton::clicked, this, &MessageBar::dismiss);

    auto *layout = new QHBoxLayout(this);
    layout->addWidget(m_icon, 0, Qt::AlignTop);
    layout->addWidget(m_text, 1);
    layout->addWidget(m_action, 0, Qt::AlignTop);
    layout->addWidget(m_copy, 0, Qt::AlignTop);
    layout->addWidget(m_close, 0, Qt::AlignTop);
    hide();
}

void MessageBar::setIcon(bool notice)
{
    const int size = style()->pixelMetric(QStyle::PM_SmallIconSize, nullptr, this);
    const QIcon icon = notice ? QIcon::fromTheme(u"help-about"_s, style()->standardIcon(QStyle::SP_MessageBoxInformation))
                              : QIcon::fromTheme(u"dialog-error"_s, style()->standardIcon(QStyle::SP_MessageBoxCritical));
    m_icon->setPixmap(icon.pixmap(size, size));
}

void MessageBar::showNotice(const QString &text, const QString &url, const QString &linkText, const QString &key)
{
    m_action->hide();
    m_copy->hide();
    m_undo = {};
    setIcon(true);
    m_key = key;
    QString html = text.toHtmlEscaped();
    if (url.startsWith(u"https://"_s) || url.startsWith(u"file:"_s))
        html += u" <a href=\"%1\">%2</a>"_s.arg(url.toHtmlEscaped(), linkText.toHtmlEscaped());
    m_text->setTextFormat(Qt::RichText);
    m_text->setText(html);
    m_plain = text;
    show();
}

void MessageBar::showError(const QString &text, const QString &url, const QString &linkText)
{
    m_action->hide();
    m_copy->show();
    m_undo = {};
    setIcon(false);
    m_key.clear();
    if (url.isEmpty()) {
        m_text->setTextFormat(Qt::PlainText);
        m_text->setText(text);
    } else {
        m_text->setTextFormat(Qt::RichText);
        m_text->setText(u"%1 <a href=\"%2\">%3</a>"_s.arg(text.toHtmlEscaped().replace(u'\n', u"<br>"_s),
                                                           url.toHtmlEscaped(), linkText.toHtmlEscaped()));
        m_plain = text;
    }
    show();
}

void MessageBar::showExplanation(const QString &text, const QString &url)
{
    m_action->hide();
    m_copy->show();
    m_undo = {};
    setIcon(false);
    m_key.clear();
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

void MessageBar::showUndo(const QString &text, const QString &button, std::function<void()> undo)
{
    setIcon(true);
    m_key.clear();
    m_text->setTextFormat(Qt::PlainText);
    m_text->setText(text);
    m_action->setText(button);
    m_action->show();
    m_copy->hide();
    m_undo = std::move(undo);
    show();
}

void MessageBar::dismiss()
{
    hide();
    m_undo = {};
    if (!m_key.isEmpty())
        emit dismissed(std::exchange(m_key, {}));
}

QString MessageBar::detailsText() const
{
    const QString details = m_details ? m_details() : QString();
    return details.isEmpty() ? text() : u"%1\n\n%2"_s.arg(text(), details);
}
