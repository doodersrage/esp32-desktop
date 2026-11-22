#include "ProblemsPanel.h"
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
        m_tree->clear();
        m_hasProblem = false;
        emit countChanged(0);
    });
    connect(core, &CoreConnection::outputExplained, this, &ProblemsPanel::add);
    connect(core, &CoreConnection::simBacktrace, this, [this](const QJsonArray &f) { addCrash(u"sim"_s, f); });
    connect(core, &CoreConnection::serialBacktrace, this, [this](const QJsonArray &f) { addCrash(u"serial"_s, f); });
    connect(core, &CoreConnection::buildFirstError, this, [this](const QJsonObject &e) {
        add(u"build"_s, QJsonArray{QJsonObject{
                            {u"level"_s, u"problem"_s},
                            {u"text"_s, tr("The build failed at %1, line %2: %3")
                                            .arg(e.value(u"file"_s).toString())
                                            .arg(e.value(u"line"_s).toInt())
                                            .arg(e.value(u"message"_s).toString())}}});
    });

    auto *bar = new QHBoxLayout;
    bar->addStretch(1);
    bar->addWidget(m_clear);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_tree, 1);
    layout->addLayout(bar);
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
