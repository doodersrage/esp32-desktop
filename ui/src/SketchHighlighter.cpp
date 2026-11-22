#include "SketchHighlighter.h"

#include "ConsoleView.h"

#include <QRegularExpression>
#include <QWidget>

using namespace Qt::StringLiterals;

namespace {
// Block states: inside a /* */ comment that goes on to the next line.
constexpr int InComment = 1;

const QRegularExpression &keywords()
{
    static const QRegularExpression re(
        u"\\b(?:alignas|auto|bool|break|case|catch|char|class|const|constexpr|continue|default|delete|do|double|"
        "else|enum|explicit|extern|false|float|for|friend|goto|if|inline|int|long|namespace|new|nullptr|operator|"
        "private|protected|public|return|short|signed|sizeof|static|struct|switch|template|this|throw|true|try|"
        "typedef|typename|union|unsigned|using|virtual|void|volatile|while|"
        "u?int(?:8|16|32|64)_t|size_t|byte|boolean|String|HIGH|LOW|INPUT|OUTPUT|INPUT_PULLUP)\\b"_s);
    return re;
}
}

SketchHighlighter::SketchHighlighter(QTextDocument *document, QWidget *colours)
    : QSyntaxHighlighter(document)
    , m_colours(colours)
{
    updateFormats();
}

void SketchHighlighter::updateFormats()
{
    const QPalette &pal = m_colours->palette();
    m_formats[Keyword] = {};
    m_formats[Keyword].setForeground(pal.link());
    m_formats[Keyword].setFontWeight(QFont::DemiBold);
    m_formats[Comment] = {};
    m_formats[Comment].setForeground(pal.color(QPalette::PlaceholderText));
    m_formats[Comment].setFontItalic(true);
    m_formats[String] = {};
    m_formats[String].setForeground(OutputLines::colour(OutputLines::Warning, pal));
    m_formats[Number] = {};
    m_formats[Number].setForeground(pal.linkVisited());
    m_formats[Preprocessor] = {};
    m_formats[Preprocessor].setForeground(pal.linkVisited());
    rehighlight();
}

void SketchHighlighter::highlightBlock(const QString &text)
{
    const qsizetype n = text.size();
    // A preprocessor line, as a whole (its comments and strings are coloured over it).
    if (text.trimmed().startsWith(u'#'))
        setFormat(0, int(n), m_formats[Preprocessor]);
    else {
        for (auto it = keywords().globalMatch(text); it.hasNext();) {
            const auto m = it.next();
            setFormat(int(m.capturedStart()), int(m.capturedLength()), m_formats[Keyword]);
        }
        static const QRegularExpression number(u"\\b(?:0[xX][0-9a-fA-F]+|0[bB][01]+|\\d+(?:\\.\\d+)?)[uUlLfF]*\\b"_s);
        for (auto it = number.globalMatch(text); it.hasNext();) {
            const auto m = it.next();
            setFormat(int(m.capturedStart()), int(m.capturedLength()), m_formats[Number]);
        }
    }
    // Strings, characters and comments, left to right: what starts first wins.
    setCurrentBlockState(0);
    qsizetype i = 0;
    if (previousBlockState() == InComment) {
        const qsizetype end = text.indexOf(u"*/"_s);
        if (end < 0) {
            setFormat(0, int(n), m_formats[Comment]);
            setCurrentBlockState(InComment);
            return;
        }
        setFormat(0, int(end + 2), m_formats[Comment]);
        i = end + 2;
    }
    while (i < n) {
        const QChar c = text.at(i);
        if (c == u'/' && i + 1 < n && text.at(i + 1) == u'/') {
            setFormat(int(i), int(n - i), m_formats[Comment]);
            return;
        }
        if (c == u'/' && i + 1 < n && text.at(i + 1) == u'*') {
            const qsizetype end = text.indexOf(u"*/"_s, i + 2);
            if (end < 0) {
                setFormat(int(i), int(n - i), m_formats[Comment]);
                setCurrentBlockState(InComment);
                return;
            }
            setFormat(int(i), int(end + 2 - i), m_formats[Comment]);
            i = end + 2;
            continue;
        }
        if (c == u'"' || c == u'\'') {
            qsizetype j = i + 1;
            while (j < n && text.at(j) != c)
                j += text.at(j) == u'\\' ? 2 : 1;
            const qsizetype end = std::min(j + 1, n);
            setFormat(int(i), int(end - i), m_formats[String]);
            i = end;
            continue;
        }
        ++i;
    }
}
