#include "ConsoleView.h"

#include "CoreConnection.h"
#include <QLocale>
#include <QGuiApplication>
#include <QClipboard>
#include "Labels.h"
#include <QMouseEvent>
#include <QFileInfo>

#include <QCheckBox>
#include <QPalette>
#include <QTextCharFormat>
#include <QTextBlock>
#include <QJsonObject>
#include <QComboBox>
#include <QDateTime>
#include <QDir>
#include <QFileDialog>
#include <QFontDatabase>
#include <QKeyEvent>
#include <QShortcut>
#include <QWheelEvent>
#include <QHBoxLayout>
#include <QIcon>
#include <QLineEdit>
#include <QSettings>
#include <QStandardPaths>
#include <QToolButton>

#include <algorithm>
#include <memory>
#include <QResizeEvent>
#include <QScrollBar>
#include <QTextCursor>

using namespace Qt::StringLiterals;

namespace {
constexpr int MaxLines = 10000;
constexpr int MinZoom = -4;
constexpr int MaxZoom = 24;
const QString ZoomSetting = u"console/zoom"_s;
}

ConsoleView::ConsoleView(QWidget *parent)
    : QPlainTextEdit(parent)
{
    setReadOnly(true);
    setLineWrapMode(QPlainTextEdit::NoWrap);
    setMaximumBlockCount(MaxLines);
    setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    m_zoom = std::clamp(QSettings().value(ZoomSetting, 0).toInt(), MinZoom, MaxZoom);
    applyZoom();
    const auto key = [this](const QKeySequence &seq, auto slot) {
        auto *s = new QShortcut(seq, this, slot);
        s->setContext(Qt::WidgetShortcut);
    };
    key(QKeySequence::ZoomIn, [this] { zoom(1); });
    key(QKeySequence(Qt::CTRL | Qt::Key_Equal), [this] { zoom(1); });
    key(QKeySequence::ZoomOut, [this] { zoom(-1); });
    key(QKeySequence(Qt::CTRL | Qt::Key_0), [this] { resetZoom(); });
    // Reading further up while output comes: it stays put, and this goes to the end.
    m_jump = new QToolButton(this);
    m_jump->setText(tr("New output ↓"));
    m_jump->setToolTip(tr("Go to the latest output and follow it"));
    m_jump->setToolButtonStyle(Qt::ToolButtonTextOnly);
    m_jump->setAutoRaise(false);
    m_jump->hide();
    connect(m_jump, &QToolButton::clicked, this, [this] { verticalScrollBar()->setValue(verticalScrollBar()->maximum()); });
    connect(verticalScrollBar(), &QScrollBar::valueChanged, this, [this](int v) {
        if (v == verticalScrollBar()->maximum())
            m_jump->hide();
    });
    // Cleared, or everything fits again.
    connect(verticalScrollBar(), &QScrollBar::rangeChanged, this, [this](int, int max) {
        if (verticalScrollBar()->value() == max)
            m_jump->hide();
    });
}

void ConsoleView::followOrOffer(bool wasAtEnd)
{
    QScrollBar *bar = verticalScrollBar();
    if (wasAtEnd) {
        bar->setValue(bar->maximum());
    } else if (m_jump->isHidden()) {
        placeJump();
        m_jump->show();
        m_jump->raise();
    }
}

void ConsoleView::placeJump()
{
    m_jump->adjustSize();
    const QRect area = viewport()->geometry();
    m_jump->move(area.right() - m_jump->width() - 8, area.bottom() - m_jump->height() - 8);
}

void ConsoleView::resizeEvent(QResizeEvent *event)
{
    QPlainTextEdit::resizeEvent(event);
    placeJump();
}

void ConsoleView::zoom(int steps)
{
    m_zoom = std::clamp(m_zoom + steps, MinZoom, MaxZoom);
    QSettings().setValue(ZoomSetting, m_zoom);
    applyZoom();
}

void ConsoleView::setZoomSteps(int steps)
{
    m_zoom = std::clamp(steps, MinZoom, MaxZoom);
    applyZoom();
}

void ConsoleView::resetZoom()
{
    zoom(-m_zoom);
}

void ConsoleView::applyZoom()
{
    QFont f = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    const qreal base = f.pointSizeF() > 0 ? f.pointSizeF() : 10.0;
    f.setPointSizeF(std::max(4.0, base + m_zoom));
    setFont(f);
}

void ConsoleView::wheelEvent(QWheelEvent *event)
{
    if (event->modifiers() & Qt::ControlModifier) {
        const int delta = event->angleDelta().y();
        if (delta != 0)
            zoom(delta > 0 ? 1 : -1);
        event->accept();
        return;
    }
    QPlainTextEdit::wheelEvent(event);
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

void ConsoleView::setHex(bool on)
{
    if (on == m_hex)
        return;
    m_hex = on;
    m_decoder.resetState();
}

QString ConsoleView::hexText(const QByteArray &bytes)
{
    // Each byte as two digits, a line break after each newline byte (0a), so text
    // still reads line by line.
    QString out;
    out.reserve(bytes.size() * 3);
    for (const char b : bytes) {
        out += u"%1"_s.arg(uchar(b), 2, 16, QLatin1Char('0'));
        out += b == '\n' ? u'\n' : u' ';
    }
    return out;
}

namespace OutputLines {

namespace {
// The tag of a line, kept with its block.
class TagData : public QTextBlockUserData {
public:
    explicit TagData(QString t)
        : tag(std::move(t))
    {
    }
    QString tag;
};
} // namespace

int state(const QString &level)
{
    if (level == u"error")
        return Error;
    if (level == u"warning")
        return Warning;
    if (level == u"info")
        return Info;
    if (level == u"debug")
        return Debug;
    if (level == u"verbose")
        return Verbose;
    return Plain;
}

QColor colour(int state, const QPalette &palette)
{
    const QColor text = palette.color(QPalette::Text);
    const bool dark = palette.color(QPalette::Base).lightness() < 128;
    switch (state) {
    case Error:
        return dark ? QColor(0xff, 0x7b, 0x72) : QColor(0xc0, 0x1c, 0x28);
    case Warning:
        return dark ? QColor(0xe3, 0xb3, 0x41) : QColor(0x9a, 0x67, 0x00);
    case Debug:
    case Verbose:
    case Notice:
        return palette.color(QPalette::PlaceholderText);
    default:
        return text;
    }
}

void mark(QTextBlock block, int state, const QString &tag, const QPalette &palette)
{
    if (!block.isValid())
        return;
    block.setUserState(state);
    if (!tag.isEmpty())
        block.setUserData(new TagData(tag)); // the block owns it
    if (state == Plain || state == Info)
        return;
    QTextCursor c(block);
    c.movePosition(QTextCursor::EndOfBlock, QTextCursor::KeepAnchor);
    QTextCharFormat f;
    f.setForeground(colour(state, palette));
    if (state == Error)
        f.setFontWeight(QFont::DemiBold);
    c.mergeCharFormat(f);
}

QString tag(const QTextBlock &block)
{
    const auto *d = static_cast<const TagData *>(block.userData());
    return d ? d->tag : QString();
}

} // namespace OutputLines

void ConsoleView::appendOutput(const QByteArray &bytes, const QJsonValue &shown, const QJsonArray &lines)
{
    if (m_hex || !shown.isString()) {
        appendBytes(bytes);
        return;
    }
    appendText(shown.toString());
    markLines(lines);
}

void ConsoleView::markLines(const QJsonArray &lines)
{
    if (lines.isEmpty())
        return;
    // The chunk's finished lines are the blocks before the last one, which holds the
    // line still coming (empty when the chunk ended with a newline).
    QTextBlock b = document()->lastBlock().previous();
    QStringList seen; // last first
    for (qsizetype i = lines.size() - 1; i >= 0 && b.isValid(); --i, b = b.previous()) {
        const QJsonObject o = lines.at(i).toObject();
        const QString tag = o.value(u"tag"_s).toString();
        OutputLines::mark(b, OutputLines::state(o.value(u"level"_s).toString()), tag, palette());
        if (!tag.isEmpty())
            seen << tag;
        if (!shows(b)) {
            b.setVisible(false);
            document()->markContentsDirty(b.position(), b.length());
        }
    }
    // New tags in the order their lines came.
    bool newTag = false;
    for (auto it = seen.crbegin(); it != seen.crend(); ++it) {
        if (!m_tags.contains(*it) && m_tags.size() < 200) {
            m_tags << *it;
            newTag = true;
        }
    }
    if (newTag)
        emit tagsChanged();
}

bool ConsoleView::shows(const QTextBlock &block) const
{
    const int s = block.userState();
    if (s == OutputLines::Notice || block == document()->lastBlock())
        return true;
    if (m_levelFilter == Problems && s != OutputLines::Error && s != OutputLines::Warning)
        return false;
    if (m_levelFilter == ErrorsOnly && s != OutputLines::Error)
        return false;
    return m_tagFilter.isEmpty() || OutputLines::tag(block) == m_tagFilter;
}

void ConsoleView::setLevelFilter(LevelFilter filter)
{
    m_levelFilter = filter;
    refilter();
}

void ConsoleView::setTagFilter(const QString &tag)
{
    m_tagFilter = tag;
    refilter();
}

void ConsoleView::refilter()
{
    for (QTextBlock b = document()->begin(); b.isValid(); b = b.next())
        b.setVisible(shows(b));
    document()->markContentsDirty(0, document()->characterCount());
    viewport()->update();
    verticalScrollBar()->setValue(verticalScrollBar()->maximum());
}

void ConsoleView::recolour()
{
    // The palette changed (light to dark): colour the marked lines for the new one.
    for (QTextBlock b = document()->begin(); b.isValid(); b = b.next()) {
        const int s = b.userState();
        if (s > OutputLines::Plain && s != OutputLines::Info) {
            QTextCursor c(b);
            c.movePosition(QTextCursor::EndOfBlock, QTextCursor::KeepAnchor);
            QTextCharFormat f;
            f.setForeground(OutputLines::colour(s, palette()));
            c.mergeCharFormat(f);
        }
    }
}

void ConsoleView::changeEvent(QEvent *event)
{
    QPlainTextEdit::changeEvent(event);
    if (event->type() == QEvent::PaletteChange)
        recolour();
}

QStringList ConsoleView::visibleLines() const
{
    QStringList out;
    for (QTextBlock b = document()->begin(); b.isValid(); b = b.next()) {
        if (b.isVisible() && !(b == document()->lastBlock() && b.text().isEmpty()))
            out << b.text();
    }
    return out;
}

void ConsoleView::appendBytes(const QByteArray &bytes)
{
    appendText(m_hex ? hexText(bytes) : m_decoder.decode(bytes));
}

void ConsoleView::appendText(QString text)
{
    text.remove(u'\r');
    if (text.isEmpty())
        return;
    if (m_timestamps || m_deltas) {
        // This computer's clock, at the moment the line's first bytes arrived, and the
        // time since the line before began.
        const QString stamp = m_timestamps ? u"[%1] "_s.arg(QTime::currentTime().toString(u"hh:mm:ss.zzz"_s)) : QString();
        QString stamped;
        for (const QChar ch : std::as_const(text)) {
            if (m_atLineStart) {
                stamped += stamp;
                if (m_deltas) {
                    stamped += m_lastLine.isValid() ? deltaText(m_lastLine.elapsed()) + u' ' : u"+0 ms "_s;
                    m_lastLine.start();
                }
            }
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
    followOrOffer(atEnd);
}

void ConsoleView::appendNotice(const QString &text)
{
    QScrollBar *bar = verticalScrollBar();
    const bool atEnd = bar->value() == bar->maximum();
    QTextCursor c(document());
    c.movePosition(QTextCursor::End);
    // A notice starts its own line: after a line still being printed, a new one.
    if (!document()->lastBlock().text().isEmpty())
        c.insertText(u"\n"_s);
    c.insertText(u"-- %1\n"_s.arg(text));
    OutputLines::mark(document()->lastBlock().previous(), OutputLines::Notice, {}, palette());
    m_atLineStart = true;
    followOrOffer(atEnd);
}

void ConsoleView::appendBacktrace(const QJsonArray &frames)
{
    if (frames.isEmpty())
        return;
    appendNotice(tr("Crash backtrace, from the build (click a line to see it):"));
    QScrollBar *bar = verticalScrollBar();
    const bool atEnd = bar->value() == bar->maximum();
    QTextCursor c(document());
    for (const QJsonValue &v : frames) {
        const QJsonObject f = v.toObject();
        const QString path = f.value(u"file"_s).toString();
        const QString file = QFileInfo(path).fileName();
        const int line = f.value(u"line"_s).toInt();
        const QString func = f.value(u"func"_s).toString(f.value(u"addr"_s).toString());
        c.movePosition(QTextCursor::End);
        if (file.isEmpty() || line <= 0) {
            c.insertText(u"--   %1\n"_s.arg(func));
        } else {
            c.insertText(u"--   "_s + tr("%1 at ").arg(func));
            QTextCharFormat link;
            link.setAnchor(true);
            link.setAnchorHref(u"frame:%1"_s.arg(m_frameLinks.size()));
            link.setFontUnderline(true);
            link.setForeground(palette().link());
            c.insertText(tr("%1, line %2").arg(file).arg(line), link);
            c.setCharFormat(QTextCharFormat());
            c.insertText(u"\n"_s);
            m_frameLinks << QJsonObject{{u"file"_s, file}, {u"line"_s, line}, {u"path"_s, path}};
        }
        OutputLines::mark(document()->lastBlock().previous(), OutputLines::Notice, {}, palette());
    }
    m_atLineStart = true;
    followOrOffer(atEnd);
}

void ConsoleView::mouseMoveEvent(QMouseEvent *event)
{
    QPlainTextEdit::mouseMoveEvent(event);
    viewport()->setCursor(anchorAt(event->position().toPoint()).startsWith(u"frame:") ? Qt::PointingHandCursor
                                                                                       : Qt::IBeamCursor);
}

void ConsoleView::mouseReleaseEvent(QMouseEvent *event)
{
    const QString href = anchorAt(event->position().toPoint());
    QPlainTextEdit::mouseReleaseEvent(event);
    if (event->button() != Qt::LeftButton || textCursor().hasSelection() || !href.startsWith(u"frame:"))
        return;
    const qsizetype i = href.mid(6).toLongLong();
    if (i >= 0 && i < m_frameLinks.size())
        emit frameActivated(m_frameLinks.at(i));
}

QString ConsoleView::bootLoopText(const QJsonObject &loop)
{
    const QString reason = loop.value(u"reason"_s).toString();
    QString text = reason.isEmpty()
        ? tr("Boot loop: the firmware reset %1 times in %2 s.").arg(loop.value(u"resets"_s).toInt()).arg(loop.value(u"seconds"_s).toInt())
        : tr("Boot loop: the firmware reset %1 times in %2 s (%3).")
              .arg(loop.value(u"resets"_s).toInt())
              .arg(loop.value(u"seconds"_s).toInt())
              .arg(reason);
    if (const QString crash = loop.value(u"crash"_s).toString(); !crash.isEmpty())
        text += u" "_s + tr("The last crash before it: %1").arg(crash);
    return text;
}

void ConsoleView::appendBootLoop(const QJsonObject &loop)
{
    appendNotice(bootLoopText(loop));
}

void ConsoleView::setDeltas(bool on)
{
    m_deltas = on;
    m_lastLine.invalidate();
}

QString ConsoleView::deltaText(qint64 ms)
{
    if (ms < 1000)
        return tr("+%1 ms").arg(ms);
    return tr("+%1 s").arg(QLocale().toString(double(ms) / 1000.0, 'f', 3));
}

QString ConsoleBar::forumText() const
{
    QString text = m_view->textCursor().selectedText().replace(QChar::ParagraphSeparator, u'\n');
    if (text.trimmed().isEmpty())
        text = m_view->visibleLines().mid(std::max<qsizetype>(0, m_view->visibleLines().size() - 200)).join(u'\n');
    return Labels::forumPost(m_context, text);
}

ConsoleBar::ConsoleBar(ConsoleView *view, CoreConnection *core, const QString &setting, QWidget *parent)
    : QWidget(parent)
    , m_view(view)
    , m_core(core)
    , m_find(new QLineEdit(this))
    , m_levels(new QComboBox(this))
    , m_tags(new QComboBox(this))
    , m_prev(new QToolButton(this))
    , m_next(new QToolButton(this))
    , m_stamps(new QCheckBox(tr("Time stamps"), this))
    , m_deltas(new QCheckBox(tr("Time between lines"), this))
    , m_forum(new QToolButton(this))
    , m_save(new QToolButton(this))
{
    m_find->setPlaceholderText(tr("Find in the output"));
    m_find->setClearButtonEnabled(true);
    m_find->setAccessibleName(tr("Find in the output"));
    m_prev->setText(tr("Previous"));
    m_prev->setToolTip(tr("Find the previous match"));
    m_next->setText(tr("Next"));
    m_next->setToolTip(tr("Find the next match (Enter)"));
    m_levels->addItem(tr("All lines"), ConsoleView::AllLines);
    m_levels->addItem(tr("Warnings and errors"), ConsoleView::Problems);
    m_levels->addItem(tr("Errors only"), ConsoleView::ErrorsOnly);
    m_levels->setAccessibleName(tr("Lines to show"));
    m_levels->setToolTip(tr("Show only the lines the firmware logged as warnings or errors"));
    connect(m_levels, &QComboBox::currentIndexChanged, this, [this] {
        m_view->setLevelFilter(ConsoleView::LevelFilter(m_levels->currentData().toInt()));
    });
    m_tags->addItem(tr("All tags"), QString());
    m_tags->setAccessibleName(tr("Tag to show"));
    m_tags->setToolTip(tr("Show only one component's log lines, by its tag (such as wifi)"));
    m_tags->setEnabled(false);
    m_tags->setSizeAdjustPolicy(QComboBox::AdjustToContents);
    connect(m_tags, &QComboBox::currentIndexChanged, this,
            [this] { m_view->setTagFilter(m_tags->currentData().toString()); });
    connect(m_view, &ConsoleView::tagsChanged, this, [this] {
        for (const QString &t : m_view->tags()) {
            if (m_tags->findData(t) < 0)
                m_tags->addItem(t, t);
        }
        m_tags->setEnabled(m_tags->count() > 1);
    });
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

    m_deltas->setToolTip(tr("Start each new line with the time since the line before it began (+12 ms), to time "
                            "loops and delays. In the simulator it is this computer's time."));
    m_deltas->setChecked(QSettings().value(setting + u"Between"_s, false).toBool());
    m_view->setDeltas(m_deltas->isChecked());
    connect(m_deltas, &QCheckBox::toggled, this, [this, setting](bool on) {
        QSettings().setValue(setting + u"Between"_s, on);
        m_view->setDeltas(on);
    });
    m_forum->setIcon(QIcon::fromTheme(u"edit-copy"_s));
    m_forum->setText(tr("Copy for a forum post"));
    m_forum->setToolButtonStyle(Qt::ToolButtonIconOnly);
    m_forum->setToolTip(tr("Copy the selected output, or the last 200 lines, as a code block for a forum post or "
                           "an issue, with the app's version and what it is from"));
    m_forum->setAccessibleName(m_forum->text());
    connect(m_forum, &QToolButton::clicked, this, [this] { QGuiApplication::clipboard()->setText(forumText()); });

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
    layout->addWidget(m_levels);
    layout->addWidget(m_tags);
    layout->addWidget(m_stamps);
    layout->addWidget(m_deltas);
    layout->addWidget(m_forum);
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

void ConsoleBar::focusFind()
{
    m_find->setFocus(Qt::ShortcutFocusReason);
    m_find->selectAll();
}

InputHistory::InputHistory(QLineEdit *input)
    : QObject(input)
    , m_input(input)
{
    input->installEventFilter(this);
}

bool InputHistory::eventFilter(QObject *watched, QEvent *event)
{
    if (watched != m_input || event->type() != QEvent::KeyPress)
        return false;
    const auto *key = static_cast<QKeyEvent *>(event);
    switch (key->key()) {
    case Qt::Key_Return:
    case Qt::Key_Enter: {
        const QString text = m_input->text();
        if (!text.isEmpty() && (m_lines.isEmpty() || m_lines.constLast() != text)) {
            m_lines << text;
            if (m_lines.size() > 100)
                m_lines.removeFirst();
        }
        m_at = -1;
        m_draft.clear();
        return false; // the panel sends it
    }
    case Qt::Key_Up:
        if (m_lines.isEmpty())
            return true;
        if (m_at == -1) {
            m_draft = m_input->text();
            m_at = int(m_lines.size()) - 1;
        } else if (m_at > 0) {
            --m_at;
        }
        m_input->setText(m_lines.at(m_at));
        return true;
    case Qt::Key_Down:
        if (m_at == -1)
            return true;
        if (m_at < m_lines.size() - 1) {
            ++m_at;
            m_input->setText(m_lines.at(m_at));
        } else {
            m_at = -1;
            m_input->setText(m_draft);
        }
        return true;
    default:
        return false;
    }
}
