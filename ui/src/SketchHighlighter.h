// Colours a sketch in the Code tab as a code view does: keywords, comments, strings,
// numbers and preprocessor lines, from the palette so light and dark themes both
// read. Rendering only; the Code tab stays a viewer.

#pragma once

#include <QSyntaxHighlighter>
#include <QTextCharFormat>

class SketchHighlighter : public QSyntaxHighlighter {
    Q_OBJECT

public:
    // `colours` gives the palette the formats come from.
    SketchHighlighter(QTextDocument *document, QWidget *colours);
    // The palette changed: new formats, everything coloured again.
    void updateFormats();

    enum Kind { Keyword, Comment, String, Number, Preprocessor };
    QTextCharFormat format(Kind kind) const { return m_formats[kind]; }

protected:
    void highlightBlock(const QString &text) override;

private:
    QWidget *m_colours;
    QTextCharFormat m_formats[5];
};
