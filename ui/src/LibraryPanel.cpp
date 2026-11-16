#include "LibraryPanel.h"

#include "CoreConnection.h"
#include "Labels.h"

#include <QDateTime>
#include <QDesktopServices>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QLocale>
#include <QToolButton>
#include <QTreeWidget>
#include <QUrl>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {

// Sorts the time column by number of minutes, not by its text.
class ProjectItem : public QTreeWidgetItem {
public:
    using QTreeWidgetItem::QTreeWidgetItem;
    bool operator<(const QTreeWidgetItem &other) const override
    {
        const int col = treeWidget() ? treeWidget()->sortColumn() : 0;
        if (col == LibraryPanel::TimeColumn)
            return data(col, Qt::UserRole).toInt() < other.data(col, Qt::UserRole).toInt();
        return QTreeWidgetItem::operator<(other);
    }
};

} // namespace

LibraryPanel::LibraryPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_filter(new QLineEdit(this))
    , m_status(new QLabel(this))
    , m_refresh(new QToolButton(this))
    , m_tree(new QTreeWidget(this))
{
    m_filter->setPlaceholderText(tr("Filter projects"));
    m_filter->setClearButtonEnabled(true);
    connect(m_filter, &QLineEdit::textChanged, this, &LibraryPanel::applyFilter);

    m_refresh->setIcon(QIcon::fromTheme(u"view-refresh"_s));
    m_refresh->setText(tr("Refresh"));
    m_refresh->setToolTip(tr("Download the project library from esp32bench.dev again"));
    m_refresh->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    connect(m_refresh, &QToolButton::clicked, this, [this] {
        setStatus(tr("Downloading the project library…"));
        m_refresh->setEnabled(false);
        m_core->refreshLibrary();
    });

    m_status->setWordWrap(true);

    m_tree->setColumnCount(ColumnCount);
    m_tree->setHeaderLabels({tr("Project"), tr("Level"), tr("Time"), tr("Chips"), tr("Firmware")});
    m_tree->setRootIsDecorated(false);
    m_tree->setUniformRowHeights(true);
    m_tree->setSortingEnabled(true);
    m_tree->sortByColumn(-1, Qt::AscendingOrder); // keep the site's order until the user sorts
    m_tree->header()->setSectionResizeMode(NameColumn, QHeaderView::Stretch);
    m_tree->header()->setStretchLastSection(false);
    for (int c = LevelColumn; c < ColumnCount; ++c)
        m_tree->header()->setSectionResizeMode(c, QHeaderView::ResizeToContents);
    connect(m_tree, &QTreeWidget::itemActivated, this, [](QTreeWidgetItem *item) {
        // Until there is a project view in the app, open the project's page.
        const QUrl url(item->data(NameColumn, UrlRole).toString());
        if (url.scheme() == u"https")
            QDesktopServices::openUrl(url);
    });

    auto *top = new QHBoxLayout;
    top->addWidget(m_filter, 1);
    top->addWidget(m_refresh);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_status);
    layout->addWidget(m_tree, 1);

    connect(m_core, &CoreConnection::libraryListed, this, &LibraryPanel::showLibrary);
    connect(m_core, &CoreConnection::libraryRefreshed, this, [this](bool ok, const QString &error) {
        m_refresh->setEnabled(m_core->state() == CoreConnection::State::Connected);
        if (!ok)
            setStatus(tr("Could not download the project library: %1").arg(error));
    });
    connect(m_core, &CoreConnection::requestFailed, this,
            [this](const QString &method, const QString &message) {
                if (method == u"library.list")
                    setStatus(tr("Could not read the project library: %1").arg(message));
            });
    connect(m_core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        updateEnabled();
        if (s == CoreConnection::State::Connected)
            m_core->listLibrary();
    });
    updateEnabled();
}

QString LibraryPanel::statusText() const
{
    return m_status->text();
}

void LibraryPanel::setStatus(const QString &text)
{
    m_status->setText(text);
}

void LibraryPanel::updateEnabled()
{
    const bool up = m_core->state() == CoreConnection::State::Connected;
    m_refresh->setEnabled(up);
    // Keep showing a library already on screen; only the button depends on the core.
    if (!up && m_tree->topLevelItemCount() == 0)
        setStatus(tr("Not connected to the core."));
}

void LibraryPanel::showLibrary(const QJsonObject &library)
{
    if (!library.value(u"available"_s).toBool()) {
        m_tree->clear();
        setStatus(tr("The project library has not been downloaded yet."));
        // Fetch it once by ourselves; after that only when the user asks.
        if (!m_autoRefreshed && m_core->state() == CoreConnection::State::Connected) {
            m_autoRefreshed = true;
            setStatus(tr("Downloading the project library…"));
            m_refresh->setEnabled(false);
            m_core->refreshLibrary();
        }
        return;
    }

    m_tree->setSortingEnabled(false); // insert in the site's order
    m_tree->clear();
    const QJsonArray projects = library.value(u"projects"_s).toArray();
    for (const QJsonValue &v : projects) {
        const QJsonObject p = v.toObject();
        auto *item = new ProjectItem(m_tree);
        item->setText(NameColumn, p.value(u"name"_s).toString());
        item->setToolTip(NameColumn, p.value(u"summary"_s).toString());
        item->setData(NameColumn, UrlRole, p.value(u"url"_s).toString());

        const QString level = p.value(u"level"_s).toString();
        item->setText(LevelColumn, level == u"Starter" ? tr("Starter")
                                   : level == u"Intermediate" ? tr("Intermediate")
                                                              : level);
        const int minutes = p.value(u"minutes"_s).toInt();
        item->setText(TimeColumn, tr("%1 min").arg(minutes));
        item->setData(TimeColumn, Qt::UserRole, minutes);

        QStringList chips;
        for (const QJsonValue &c : p.value(u"chips"_s).toArray())
            chips << Labels::chipName(c.toString());
        item->setText(ChipsColumn, chips.join(u", "_s));

        const bool prebuilt = p.value(u"firmware"_s).toString() == u"prebuilt";
        item->setText(FirmwareColumn, prebuilt ? tr("Prebuilt") : tr("Arduino sketch"));
        item->setToolTip(FirmwareColumn, prebuilt ? tr("Flashed as a ready-made image; no build needed.")
                                                  : tr("Built from source with arduino-cli."));

        // Searchable text, lower-cased once.
        QStringList words{p.value(u"name"_s).toString(), p.value(u"summary"_s).toString(),
                          p.value(u"id"_s).toString()};
        for (const QJsonValue &t : p.value(u"tags"_s).toArray())
            words << t.toString();
        words << chips;
        item->setData(NameColumn, Qt::UserRole, words.join(u' ').toLower());
    }
    m_tree->setSortingEnabled(true);

    const qint64 fetched = library.value(u"fetchedAt"_s).toInteger();
    const QString when = fetched > 0
        ? QLocale().toString(QDateTime::fromSecsSinceEpoch(fetched), QLocale::ShortFormat)
        : tr("unknown");
    setStatus(tr("Projects: %1. Downloaded %2.").arg(projects.size()).arg(when));
    applyFilter();
}

void LibraryPanel::applyFilter()
{
    const QStringList terms = m_filter->text().toLower().split(u' ', Qt::SkipEmptyParts);
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
        QTreeWidgetItem *item = m_tree->topLevelItem(i);
        const QString text = item->data(NameColumn, Qt::UserRole).toString();
        const bool match = std::all_of(terms.begin(), terms.end(),
                                       [&](const QString &t) { return text.contains(t); });
        item->setHidden(!match);
    }
}
