// A read-only view of a sketch with a gutter: line numbers, a dot on each line with a
// breakpoint, and an arrow on the line the firmware is paused at. Renders only; a
// click in the gutter is reported for the debugger to act on.

#pragma once

#include <QPlainTextEdit>
#include <QSet>

class CodeView : public QPlainTextEdit {
    Q_OBJECT

public:
    explicit CodeView(QWidget *parent = nullptr);

    // 1-based line numbers.
    void setBreakpointLines(const QSet<int> &lines);
    QSet<int> breakpointLines() const { return m_breakpoints; }
    // The line the firmware is paused at, or 0 for none; shown and scrolled to.
    void setPausedLine(int line);
    int pausedLine() const { return m_paused; }
    int gutterWidth() const;
    // The line at a point in the gutter's coordinates, or 0.
    int lineAt(int y) const;

signals:
    void gutterClicked(int line);

protected:
    void resizeEvent(QResizeEvent *event) override;
    void changeEvent(QEvent *event) override;

private:
    friend class CodeGutter;
    void paintGutter(QPaintEvent *event);
    void updateGutter(const QRect &rect, int dy);
    void highlightPaused();

    QWidget *m_gutter;
    QSet<int> m_breakpoints;
    int m_paused = 0;
};
