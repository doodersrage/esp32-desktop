#include "CodeView.h"

#include "ConsoleView.h"

#include <QFontDatabase>
#include <QMouseEvent>
#include <QPainter>
#include <QTextBlock>

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
