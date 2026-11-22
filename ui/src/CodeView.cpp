#include "CodeView.h"

#include "ConsoleView.h"

#include <QContextMenuEvent>
#include <QFontDatabase>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QTextBlock>

using namespace Qt::StringLiterals;

class CodeGutter : public QWidget {
public:
    explicit CodeGutter(CodeView *view)
        : QWidget(view)
        , m_view(view)
    {
        setCursor(Qt::PointingHandCursor);
    }
    QSize sizeHint() const override { return {m_view->gutterWidth(), 0}; }

protected:
    void paintEvent(QPaintEvent *event) override { m_view->paintGutter(event); }
    void contextMenuEvent(QContextMenuEvent *event) override
    {
        if (const int line = m_view->lineAt(event->pos().y()); line > 0) {
            QMenu *menu = m_view->lineMenu(line);
            menu->setAttribute(Qt::WA_DeleteOnClose);
            menu->popup(event->globalPos());
        }
    }
    void mousePressEvent(QMouseEvent *event) override
    {
        if (event->button() == Qt::LeftButton) {
            if (const int line = m_view->lineAt(event->position().toPoint().y()); line > 0)
                emit m_view->gutterClicked(line);
        }
    }

private:
    CodeView *m_view;
};

CodeView::CodeView(QWidget *parent)
    : QPlainTextEdit(parent)
    , m_gutter(new CodeGutter(this))
{
    setReadOnly(true);
    setLineWrapMode(QPlainTextEdit::NoWrap);
    setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    m_gutter->setToolTip(tr("Click beside a line to add or remove a breakpoint there"));
    m_gutter->setAccessibleName(tr("Line numbers and breakpoints"));
    m_gutter->setObjectName(QStringLiteral("gutter"));
    connect(this, &QPlainTextEdit::blockCountChanged, this, [this] { setViewportMargins(gutterWidth(), 0, 0, 0); });
    connect(this, &QPlainTextEdit::updateRequest, this, &CodeView::updateGutter);
    setViewportMargins(gutterWidth(), 0, 0, 0);
}

int CodeView::gutterWidth() const
{
    int digits = 1;
    for (int n = std::max(1, blockCount()); n >= 10; n /= 10)
        ++digits;
    // Room for the dot or arrow, then the numbers.
    return fontMetrics().height() + 6 + fontMetrics().horizontalAdvance(QLatin1Char('9')) * std::max(2, digits);
}

QMenu *CodeView::lineMenu(int line)
{
    auto *menu = new QMenu(this);
    menu->addAction(QIcon::fromTheme(u"media-record"_s), tr("Add or remove a breakpoint on line %1").arg(line), this,
                    [this, line] { emit gutterClicked(line); });
    QAction *run = menu->addAction(QIcon::fromTheme(u"media-playback-start"_s), tr("Run to line %1").arg(line), this,
                                   [this, line] { emit runToRequested(line); });
    run->setEnabled(m_runTo);
    if (!m_runTo)
        run->setToolTip(tr("While debugging, when the firmware is paused"));
    menu->setToolTipsVisible(true);
    return menu;
}

void CodeView::contextMenuEvent(QContextMenuEvent *event)
{
    QMenu *menu = createStandardContextMenu(event->pos());
    menu->setAttribute(Qt::WA_DeleteOnClose);
    const int line = cursorForPosition(event->pos()).blockNumber() + 1;
    QMenu *lines = lineMenu(line);
    menu->addSeparator();
    for (QAction *a : lines->actions()) {
        a->setParent(menu);
        menu->addAction(a);
    }
    lines->deleteLater();
    menu->setToolTipsVisible(true);
    menu->popup(event->globalPos());
}

bool CodeView::findText(const QString &text, bool backward)
{
    if (text.isEmpty())
        return false;
    const QTextDocument::FindFlags flags = backward ? QTextDocument::FindBackward : QTextDocument::FindFlags();
    if (find(text, flags))
        return true;
    // Round from the other end.
    const QTextCursor saved = textCursor();
    QTextCursor c(document());
    c.movePosition(backward ? QTextCursor::End : QTextCursor::Start);
    setTextCursor(c);
    if (find(text, flags))
        return true;
    setTextCursor(saved);
    return false;
}

bool CodeView::goToLine(int line)
{
    const QTextBlock block = document()->findBlockByNumber(line - 1);
    if (line < 1 || !block.isValid())
        return false;
    QTextCursor c(block);
    c.movePosition(QTextCursor::EndOfBlock, QTextCursor::KeepAnchor);
    setTextCursor(c);
    centerCursor();
    return true;
}

int CodeView::lineAt(int y) const
{
    for (QTextBlock b = firstVisibleBlock(); b.isValid(); b = b.next()) {
        const QRectF r = blockBoundingGeometry(b).translated(contentOffset());
        if (r.top() > y)
            break;
        if (b.isVisible() && y < r.bottom())
            return b.blockNumber() + 1;
    }
    return 0;
}

void CodeView::setBreakpointLines(const QSet<int> &lines)
{
    m_breakpoints = lines;
    m_gutter->update();
}

void CodeView::setPausedLine(int line)
{
    m_paused = line;
    highlightPaused();
    if (line > 0) {
        const QTextBlock b = document()->findBlockByNumber(line - 1);
        if (b.isValid()) {
            QTextCursor c(b);
            setTextCursor(c);
            centerCursor();
        }
    }
    m_gutter->update();
}

void CodeView::highlightPaused()
{
    QList<QTextEdit::ExtraSelection> marks;
    const QTextBlock b = document()->findBlockByNumber(m_paused - 1);
    if (m_paused > 0 && b.isValid()) {
        QTextEdit::ExtraSelection mark;
        QColor c = palette().color(QPalette::Highlight);
        c.setAlpha(60);
        mark.format.setBackground(c);
        mark.format.setProperty(QTextFormat::FullWidthSelection, true);
        mark.cursor = QTextCursor(b);
        marks << mark;
    }
    setExtraSelections(marks);
}

void CodeView::resizeEvent(QResizeEvent *event)
{
    QPlainTextEdit::resizeEvent(event);
    const QRect r = contentsRect();
    m_gutter->setGeometry(r.left(), r.top(), gutterWidth(), r.height());
}

void CodeView::changeEvent(QEvent *event)
{
    QPlainTextEdit::changeEvent(event);
    if (event->type() == QEvent::PaletteChange || event->type() == QEvent::FontChange) {
        setViewportMargins(gutterWidth(), 0, 0, 0);
        highlightPaused();
    }
}

void CodeView::updateGutter(const QRect &rect, int dy)
{
    if (dy != 0)
        m_gutter->scroll(0, dy);
    else
        m_gutter->update(0, rect.y(), m_gutter->width(), rect.height());
}

void CodeView::paintGutter(QPaintEvent *event)
{
    QPainter p(m_gutter);
    const QPalette pal = palette();
    p.fillRect(event->rect(), pal.color(QPalette::AlternateBase));
    const int mark = fontMetrics().height();
    for (QTextBlock b = firstVisibleBlock(); b.isValid(); b = b.next()) {
        const QRectF r = blockBoundingGeometry(b).translated(contentOffset());
        if (r.top() > event->rect().bottom())
            break;
        if (!b.isVisible() || r.bottom() < event->rect().top())
            continue;
        const int line = b.blockNumber() + 1;
        const int top = qRound(r.top());
        if (m_breakpoints.contains(line)) {
            p.setRenderHint(QPainter::Antialiasing);
            p.setPen(Qt::NoPen);
            p.setBrush(OutputLines::colour(OutputLines::Error, pal));
            p.drawEllipse(QRectF(3, top + mark * 0.2, mark * 0.6, mark * 0.6));
        }
        if (line == m_paused) {
            p.setRenderHint(QPainter::Antialiasing);
            p.setPen(Qt::NoPen);
            p.setBrush(pal.color(QPalette::Highlight));
            const QPolygonF arrow{QPointF(3, top + mark * 0.15), QPointF(3 + mark * 0.7, top + mark * 0.5),
                                  QPointF(3, top + mark * 0.85)};
            p.drawPolygon(arrow);
        }
        p.setPen(line == m_paused ? pal.color(QPalette::Text) : pal.color(QPalette::PlaceholderText));
        p.drawText(QRect(0, top, m_gutter->width() - 4, mark), Qt::AlignRight | Qt::AlignVCenter,
                   QString::number(line));
    }
}
