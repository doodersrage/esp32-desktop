#include "ProblemsPanel.h"
#include <QUrlQuery>
#include <QMenu>
#include <QRegularExpression>
#include <QGuiApplication>
#include <QClipboard>
#include <QComboBox>

#include "ConsoleView.h"
#include <QFileInfo>

#include "CoreConnection.h"

#include <QDesktopServices>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QJsonObject>
#include <QPushButton>
#include <QTime>
#include <QTreeWidget>
#include <QUrl>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

ProblemsPanel::ProblemsPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_tree(new QTreeWidget(this))
    , m_clear(new QPushButton(QIcon::fromTheme(u"edit-clear"_s), tr("Clear"), this))
{
    m_tree->setColumnCount(ColumnCount);
    m_tree->setHeaderLabels({tr("Time"), tr("From"), tr("What it means")});
    m_tree->setRootIsDecorated(false);
    m_tree->setWordWrap(true);
    m_tree->setAccessibleName(tr("Problems"));
    m_tree->header()->setSectionResizeMode(TimeColumn, QHeaderView::ResizeToContents);
    m_tree->header()->setSectionResizeMode(SourceColumn, QHeaderView::ResizeToContents);
    m_tree->header()->setStretchLastSection(true);
    m_tree->setToolTip(tr("Double-click one to read about it on esp32bench.dev"));
    connect(m_tree, &QTreeWidget::itemActivated, this, [this](QTreeWidgetItem *item) {
        if (const QJsonObject at = item->data(TextColumn, LocationRole).toJsonObject(); !at.isEmpty()) {
            emit locationActivated(at);
            return;
        }
        const QUrl url(item->data(TextColumn, UrlRole).toString());
        if (url.scheme() == u"https")
            QDesktopServices::openUrl(url);
    });
    m_clear->setToolTip(tr("Clear the list"));
    connect(m_clear, &QPushButton::clicked, this, [this] {
        // The kind shown, or all of them.
        for (int i = m_tree->topLevelItemCount() - 1; i >= 0; --i) {
            if (!m_tree->topLevelItem(i)->isHidden())
                delete m_tree->takeTopLevelItem(i);
        }
        if (m_tree->topLevelItemCount() == 0)
            m_hasProblem = false;
        emit countChanged(m_tree->topLevelItemCount());
    });
    // Only one kind: the build's, the simulator's or the board's.
    m_filter = new QComboBox(this);
    m_filter->setAccessibleName(tr("Show problems from"));
    m_filter->addItem(tr("Everywhere"), QString());
    m_filter->addItem(tr("Build"), u"build"_s);
    m_filter->addItem(tr("Simulator"), u"sim"_s);
    m_filter->addItem(tr("Board"), u"serial"_s);
    connect(m_filter, &QComboBox::currentIndexChanged, this, [this] { showOnly(m_filter->currentData().toString()); });
    connect(core, &CoreConnection::outputExplained, this, &ProblemsPanel::add);
    connect(core, &CoreConnection::simBacktrace, this, [this](const QJsonArray &f) { addCrash(u"sim"_s, f); });
    for (const auto &[signal, source] : {std::pair{&CoreConnection::simBootLoop, u"sim"_s},
                                         std::pair{&CoreConnection::serialBootLoop, u"serial"_s}}) {
        connect(core, signal, this, [this, source](const QJsonObject &loop) {
            add(source, QJsonArray{QJsonObject{{u"level"_s, u"problem"_s}, {u"text"_s, ConsoleView::bootLoopText(loop)}}});
            // The crash's own line, else the reset's reason.
            const QString words = loop.value(u"crash"_s).toString(loop.value(u"reason"_s).toString());
            if (QTreeWidgetItem *item = m_tree->topLevelItem(m_tree->topLevelItemCount() - 1))
                item->setData(TextColumn, SearchRole, words);
        });
    }
    connect(core, &CoreConnection::serialBacktrace, this, [this](const QJsonArray &f) { addCrash(u"serial"_s, f); });
    connect(core, &CoreConnection::buildFirstError, this, [this](const QJsonObject &e) {
        add(u"build"_s, QJsonArray{QJsonObject{
                            {u"level"_s, u"problem"_s},
                            {u"text"_s, tr("The build failed at %1, line %2: %3")
                                            .arg(e.value(u"file"_s).toString())
                                            .arg(e.value(u"line"_s).toInt())
                                            .arg(e.value(u"message"_s).toString())}}});
        if (QTreeWidgetItem *item = m_tree->topLevelItem(m_tree->topLevelItemCount() - 1))
            item->setData(TextColumn, SearchRole, e.value(u"message"_s).toString());
    });
    // Right-click: look an error up on esp32bench.dev, by its own words only.
    m_tree->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(m_tree, &QWidget::customContextMenuRequested, this, [this](const QPoint &at) {
        QTreeWidgetItem *item = m_tree->itemAt(at);
        if (!item)
            return;
        QMenu menu(this);
        const QString words = searchTerms(item->data(TextColumn, SearchRole).toString());
        QAction *search = menu.addAction(QIcon::fromTheme(u"internet-web-browser"_s), tr("Look it up on esp32bench.dev"),
                                         this, [words] { QDesktopServices::openUrl(searchUrl(words)); });
        search->setEnabled(!words.isEmpty());
        menu.addAction(QIcon::fromTheme(u"edit-copy"_s), tr("Copy"), this,
                       [item] { QGuiApplication::clipboard()->setText(item->text(TextColumn)); });
        menu.exec(m_tree->viewport()->mapToGlobal(at));
    });

    auto *bar = new QHBoxLayout;
    bar->addWidget(m_filter);
    bar->addStretch(1);
    bar->addWidget(m_clear);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_tree, 1);
    layout->addLayout(bar);
}

QString ProblemsPanel::searchTerms(const QString &text)
{
    // "Guru Meditation Error: Core 1 panic'ed (LoadProhibited). …": the exception.
    static const QRegularExpression exception(u"\\(([A-Za-z]+)\\)"_s);
    if (text.contains(u"Guru Meditation"_s)) {
        if (const auto m = exception.match(text); m.hasMatch())
            return m.captured(1);
    }
    // A compiler message: without "error:" and the names it quotes.
    QString t = text;
    t.remove(QRegularExpression(u"^.*?\\b(?:fatal )?error:\\s*"_s));
    t.remove(QRegularExpression(u"['‘\"][^'’\"]*['’\"]"_s));
    t.remove(QRegularExpression(u"[^\\w\\s_-]"_s));
    return t.split(u' ', Qt::SkipEmptyParts).mid(0, 6).join(u' ');
}

void ProblemsPanel::showOnly(const QString &source)
{
    m_only = source;
    const int i = m_filter->findData(source);
    if (i >= 0 && i != m_filter->currentIndex())
        m_filter->setCurrentIndex(i);
    for (int n = 0; n < m_tree->topLevelItemCount(); ++n) {
        QTreeWidgetItem *item = m_tree->topLevelItem(n);
        item->setHidden(!source.isEmpty() && item->data(SourceColumn, SourceRole).toString() != source);
    }
    m_clear->setToolTip(source.isEmpty() ? tr("Clear the list") : tr("Clear the ones shown"));
}

QUrl ProblemsPanel::searchUrl(const QString &words)
{
    QUrl url(u"https://esp32bench.dev/search/"_s);
    QUrlQuery query;
    query.addQueryItem(u"q"_s, searchTerms(words));
    url.setQuery(query);
    return url;
}

void ProblemsPanel::addCrash(const QString &source, const QJsonArray &frames)
{
    // The sketch's own frame if there is one, else the innermost named one.
    QJsonObject at;
    for (const QJsonValue &v : frames) {
        if (v.toObject().value(u"sketch"_s).toBool()) {
            at = v.toObject();
            break;
        }
    }
    for (const QJsonValue &v : frames) {
        if (at.isEmpty() && v.toObject().contains(u"func"_s))
            at = v.toObject();
    }
    if (at.isEmpty() && !frames.isEmpty())
        at = frames.first().toObject();
    const QString path = at.value(u"file"_s).toString();
    const QString file = QFileInfo(path).fileName();
    const int line = at.value(u"line"_s).toInt();
    const QString func = at.value(u"func"_s).toString(at.value(u"addr"_s).toString());
    const QString text = !file.isEmpty() && line > 0 ? tr("Crashed in %1 (%2, line %3)").arg(func, file).arg(line)
                                                     : tr("Crashed in %1").arg(func);
    add(source, QJsonArray{QJsonObject{{u"level"_s, u"problem"_s}, {u"text"_s, text}}});
    QTreeWidgetItem *item = m_tree->topLevelItem(m_tree->topLevelItemCount() - 1);
    if (item && !file.isEmpty() && line > 0) {
        item->setData(TextColumn, LocationRole,
                      QJsonObject{{u"file"_s, file}, {u"line"_s, line}, {u"path"_s, path}});
        item->setToolTip(TextColumn, tr("%1\nDouble-click to see the line.").arg(text));
    }
}

void ProblemsPanel::add(const QString &source, const QJsonArray &findings)
{
    const QString from = source == u"sim" ? tr("Simulator") : source == u"serial" ? tr("Board") : tr("Build");
    for (const QJsonValue &v : findings) {
        const QJsonObject f = v.toObject();
        const bool problem = f.value(u"level"_s).toString() == u"problem";
        auto *item = new QTreeWidgetItem(m_tree);
        item->setText(TimeColumn, QTime::currentTime().toString(u"hh:mm:ss"_s));
        item->setText(SourceColumn, from);
        item->setData(SourceColumn, SourceRole, source);
        item->setHidden(!m_only.isEmpty() && m_only != source);
        item->setText(TextColumn, f.value(u"text"_s).toString());
        item->setIcon(TextColumn, QIcon::fromTheme(problem ? u"dialog-error"_s : u"help-about"_s));
        item->setData(TextColumn, UrlRole, f.value(u"url"_s).toString());
        item->setToolTip(TextColumn, f.value(u"text"_s).toString());
        m_tree->scrollToItem(item);
        if (problem && !m_hasProblem) {
            m_hasProblem = true;
            emit firstProblem();
        }
    }
    if (!findings.isEmpty())
        emit countChanged(m_tree->topLevelItemCount());
}
