#include "ConsoleView.h"

#include "CoreConnection.h"

#include <QCheckBox>
#include <QDateTime>
#include <QDir>
#include <QFileDialog>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QIcon>
#include <QLineEdit>
#include <QSettings>
#include <QStandardPaths>
#include <QToolButton>

#include <memory>
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

void ConsoleView::setTimestamps(bool on)
{
    m_timestamps = on;
}

bool ConsoleView::findText(const QString &text, bool backward)
{
    if (text.isEmpty())
        return false;
    const QTextDocument::FindFlags flags = backward ? QTextDocument::FindBackward : QTextDocument::FindFlags();
    if (find(text, flags))
        return true;
    // Wrap round: from the other end.
    QTextCursor c = textCursor();
    c.movePosition(backward ? QTextCursor::End : QTextCursor::Start);
    setTextCursor(c);
    return find(text, flags);
}

void ConsoleView::appendBytes(const QByteArray &bytes)
{
    QString text = m_decoder.decode(bytes);
    text.remove(u'\r');
    if (text.isEmpty())
        return;
    if (m_timestamps) {
        // This computer's clock, at the moment the line's first bytes arrived.
        const QString stamp = u"[%1] "_s.arg(QTime::currentTime().toString(u"hh:mm:ss.zzz"_s));
        QString stamped;
        for (const QChar ch : std::as_const(text)) {
            if (m_atLineStart)
                stamped += stamp;
            stamped += ch;
            m_atLineStart = ch == u'\n';
        }
        text = stamped;
    } else {
        m_atLineStart = text.endsWith(u'\n');
    }
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
    m_atLineStart = true;
    verticalScrollBar()->setValue(verticalScrollBar()->maximum());
}

ConsoleBar::ConsoleBar(ConsoleView *view, CoreConnection *core, const QString &setting, QWidget *parent)
    : QWidget(parent)
    , m_view(view)
    , m_core(core)
    , m_find(new QLineEdit(this))
    , m_prev(new QToolButton(this))
    , m_next(new QToolButton(this))
    , m_stamps(new QCheckBox(tr("Time stamps"), this))
    , m_save(new QToolButton(this))
{
    m_find->setPlaceholderText(tr("Find in the output"));
    m_find->setClearButtonEnabled(true);
    m_prev->setText(tr("Previous"));
    m_prev->setToolTip(tr("Find the previous match"));
    m_next->setText(tr("Next"));
    m_next->setToolTip(tr("Find the next match (Enter)"));
    const auto findNext = [this](bool backward) {
        const bool found = m_view->findText(m_find->text(), backward);
        m_find->setToolTip(found || m_find->text().isEmpty() ? QString() : tr("Not found"));
    };
    connect(m_find, &QLineEdit::returnPressed, this, [findNext] { findNext(false); });
    connect(m_next, &QToolButton::clicked, this, [findNext] { findNext(false); });
    connect(m_prev, &QToolButton::clicked, this, [findNext] { findNext(true); });

    m_stamps->setToolTip(tr("Start each new line with this computer's time. In the simulator this is not "
                            "the simulated chip's time."));
    m_stamps->setChecked(QSettings().value(setting, false).toBool());
    m_view->setTimestamps(m_stamps->isChecked());
    connect(m_stamps, &QCheckBox::toggled, this, [this, setting](bool on) {
        QSettings().setValue(setting, on);
        m_view->setTimestamps(on);
    });

    m_save->setIcon(QIcon::fromTheme(u"document-save"_s));
    m_save->setText(tr("Save…"));
    m_save->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_save->setToolTip(tr("Save the output to a text file"));
    connect(m_save, &QToolButton::clicked, this, [this] {
        const QString dir = QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation);
        const QString path = QFileDialog::getSaveFileName(
            this, tr("Save the output"),
            QDir(dir).filePath(u"esp32bench-%1.txt"_s.arg(QDateTime::currentDateTime().toString(u"yyyyMMdd-HHmmss"_s))),
            tr("Text files (*.txt);;All files (*)"));
        if (!path.isEmpty())
            saveTo(path);
    });

    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(m_find, 1);
    layout->addWidget(m_prev);
    layout->addWidget(m_next);
    layout->addWidget(m_stamps);
    layout->addWidget(m_save);
}

void ConsoleBar::saveTo(const QString &path)
{
    // Say how it went in the console itself, once, for this file.
    auto ok = std::make_shared<QMetaObject::Connection>();
    auto failed = std::make_shared<QMetaObject::Connection>();
    *ok = connect(m_core, &CoreConnection::consoleSaved, this, [this, path, ok, failed](const QString &saved) {
        if (saved != path)
            return;
        disconnect(*ok);
        disconnect(*failed);
        m_view->appendNotice(tr("Saved the output to %1.").arg(path));
    });
    *failed = connect(m_core, &CoreConnection::requestFailed, this,
                      [this, path, ok, failed](const QString &method, const QString &message) {
                          if (method != u"console.save")
                              return;
                          disconnect(*ok);
                          disconnect(*failed);
                          m_view->appendNotice(tr("Could not save the output: %1").arg(message));
                      });
    m_core->saveConsole(path, m_view->toPlainText());
}
