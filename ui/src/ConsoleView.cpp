#include "ConsoleView.h"

#include <QFontDatabase>
#include <QScrollBar>
#include <QTextCursor>

using namespace Qt::StringLiterals;

namespace {
constexpr int MaxLines = 10000;
}

ConsoleView::ConsoleView(QWidget *parent)
    : QPlainTextEdit(parent)
{
    setReadOnly(true);
    setLineWrapMode(QPlainTextEdit::NoWrap);
    setMaximumBlockCount(MaxLines);
    setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
}

void ConsoleView::resetDecoder()
{
    m_decoder.resetState();
}

void ConsoleView::appendBytes(const QByteArray &bytes)
{
    QString text = m_decoder.decode(bytes);
    text.remove(u'\r');
    if (text.isEmpty())
        return;
    QScrollBar *bar = verticalScrollBar();
    const bool atEnd = bar->value() == bar->maximum();
    QTextCursor c(document());
    c.movePosition(QTextCursor::End);
    c.insertText(text);
    if (atEnd)
        bar->setValue(bar->maximum());
}

void ConsoleView::appendNotice(const QString &text)
{
    const QString current = toPlainText();
    QTextCursor c(document());
    c.movePosition(QTextCursor::End);
    if (!current.isEmpty() && !current.endsWith(u'\n'))
        c.insertText(u"\n"_s);
    c.insertText(u"-- %1\n"_s.arg(text));
    verticalScrollBar()->setValue(verticalScrollBar()->maximum());
}
