// A read-only view of a sketch with a gutter: line numbers, a dot on each line with a
// breakpoint, and an arrow on the line the firmware is paused at. Renders only; a
// click in the gutter is reported for the debugger to act on.

#pragma once

#include <QPlainTextEdit>
#include <QSet>

class QMenu;
class SketchHighlighter;

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
    // The sketch's colours (keywords, comments, strings…).
    SketchHighlighter *highlighter() const { return m_highlighter; }
    // The line at a point in the gutter's coordinates, or 0.
    int lineAt(int y) const;
    // Select the next (or previous) match after the cursor, wrapping round; false if
    // there is none.
    bool findText(const QString &text, bool backward = false);
    // Select a line and scroll to it; false if there is no such line.
    bool goToLine(int line);
    // Whether the right-click menu's Run to this line works, and hovering a name asks
    // for its value (wordHovered): while the debugger is paused.
    void setRunToEnabled(bool on) { m_runTo = on; }
    // The name (an identifier) at a point in the viewport, or empty.
    QString wordAt(const QPoint &pos) const;
    // The right-click menu for a line (also the gutter's), for tests.
    QMenu *lineMenu(int line);

signals:
    void gutterClicked(int line);
    void runToRequested(int line);
    // The line menu's Breakpoint with a condition.
    void conditionRequested(int line);
    // The mouse rests on a name while paused: show its value at `globalPos`.
    void wordHovered(const QString &word, const QPoint &globalPos);

protected:
    void contextMenuEvent(QContextMenuEvent *event) override;
    bool viewportEvent(QEvent *event) override;
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
    bool m_runTo = false;
    SketchHighlighter *m_highlighter;
};
