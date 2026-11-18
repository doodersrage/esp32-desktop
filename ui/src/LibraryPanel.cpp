#include "LibraryPanel.h"

#include "CoreConnection.h"
#include "Labels.h"

#include <QDateTime>
#include <QFileDialog>
#include <QMenu>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QLocale>
#include <QToolButton>
#include <QTreeWidget>
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
    , m_open(new QToolButton(this))
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

    m_open->setIcon(QIcon::fromTheme(u"document-open-folder"_s));
    m_open->setText(tr("Open ESP-IDF project…"));
    m_open->setToolTip(tr("Add one of your own ESP-IDF project folders, to build with ESP-IDF, flash and simulate"));
    m_open->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    connect(m_open, &QToolButton::clicked, this, &LibraryPanel::openFolder);

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
    connect(m_tree, &QTreeWidget::currentItemChanged, this, [this](QTreeWidgetItem *item) {
        if (item)
            emit projectSelected(item->data(NameColumn, IdRole).toString());
    });
    m_tree->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(m_tree, &QTreeWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        QTreeWidgetItem *item = m_tree->itemAt(pos);
        if (!item || !item->data(NameColumn, LocalRole).toBool())
            return;
        const QString id = item->data(NameColumn, IdRole).toString();
        QMenu menu(this);
        menu.addAction(QIcon::fromTheme(u"list-remove"_s), tr("Forget this project"), this,
                       [this, id] { m_core->removeLocal(id); });
        menu.exec(m_tree->viewport()->mapToGlobal(pos));
    });

    auto *top = new QHBoxLayout;
    top->addWidget(m_filter, 1);
    top->addWidget(m_refresh);
    top->addWidget(m_open);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_status);
    layout->addWidget(m_tree, 1);

    connect(m_core, &CoreConnection::libraryListed, this, &LibraryPanel::showLibrary);
    connect(m_core, &CoreConnection::localListed, this,
            [this](const QJsonArray &projects, const QJsonObject &) { showLocal(projects); });
    connect(m_core, &CoreConnection::localOpened, this, [this](const QJsonObject &project) {
        m_pendingSelect = project.value(u"id"_s).toString();
        m_core->listLocal();
    });
    connect(m_core, &CoreConnection::libraryRefreshed, this, [this](bool ok, const QString &error) {
        m_refresh->setEnabled(m_core->state() == CoreConnection::State::Connected);
        if (!ok)
            setStatus(tr("Could not download the project library: %1").arg(error));
    });
    connect(m_core, &CoreConnection::requestFailed, this,
            [this](const QString &method, const QString &message) {
                if (method == u"library.list")
                    setStatus(tr("Could not read the project library: %1").arg(message));
                else if (method == u"local.open")
                    setStatus(tr("Could not open that folder: %1").arg(message));
            });
    connect(m_core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        updateEnabled();
        if (s == CoreConnection::State::Connected) {
            m_core->listLibrary();
            m_core->listLocal();
        }
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
    m_open->setEnabled(up);
    // Keep showing a library already on screen; only the button depends on the core.
    if (!up && m_tree->topLevelItemCount() == 0)
        setStatus(tr("Not connected to the core."));
}

void LibraryPanel::showLibrary(const QJsonObject &library)
{
    if (!library.value(u"available"_s).toBool()) {
        m_tree->clear();
        addLocalItems();
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
        item->setData(NameColumn, IdRole, p.value(u"id"_s).toString());

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
    addLocalItems();
    m_tree->setSortingEnabled(true);

    const qint64 fetched = library.value(u"fetchedAt"_s).toInteger();
    const QString when = fetched > 0
        ? QLocale().toString(QDateTime::fromSecsSinceEpoch(fetched), QLocale::ShortFormat)
        : tr("unknown");
    setStatus(tr("Projects: %1. Downloaded %2.").arg(projects.size()).arg(when));
    applyFilter();
    if (!m_pendingSelect.isEmpty())
        selectProject(std::exchange(m_pendingSelect, {}));
}

void LibraryPanel::openFolder()
{
    const QString dir = QFileDialog::getExistingDirectory(this, tr("Open an ESP-IDF project folder"));
    if (!dir.isEmpty())
        m_core->openLocal(dir); // the core checks it is an ESP-IDF project
}

void LibraryPanel::showLocal(const QJsonArray &projects)
{
    m_local = projects;
    const QString current = m_tree->currentItem() ? m_tree->currentItem()->data(NameColumn, IdRole).toString()
                                                   : QString();
    m_tree->setSortingEnabled(false);
    for (int i = m_tree->topLevelItemCount() - 1; i >= 0; --i) {
        if (m_tree->topLevelItem(i)->data(NameColumn, LocalRole).toBool())
            delete m_tree->takeTopLevelItem(i);
    }
    addLocalItems();
    m_tree->setSortingEnabled(true);
    applyFilter();
    if (!m_pendingSelect.isEmpty())
        selectProject(std::exchange(m_pendingSelect, {}));
    else if (!current.isEmpty() && current.startsWith(u"local-")) {
        // The one shown was forgotten: nothing is selected now.
        bool still = false;
        for (const QJsonValue &v : projects)
            still = still || v.toObject().value(u"id"_s).toString() == current;
        if (!still)
            m_tree->setCurrentItem(nullptr);
    }
}

// The local projects at the top, before the library's.
void LibraryPanel::addLocalItems()
{
    int at = 0;
    for (const QJsonValue &v : m_local) {
        const QJsonObject p = v.toObject();
        auto *item = new ProjectItem;
        item->setText(NameColumn, p.value(u"name"_s).toString());
        item->setToolTip(NameColumn, p.value(u"path"_s).toString());
        item->setData(NameColumn, IdRole, p.value(u"id"_s).toString());
        item->setData(NameColumn, LocalRole, true);
        item->setText(ChipsColumn, QStringList{Labels::chipName(u"esp32"_s), Labels::chipName(u"esp32s3"_s),
                                               Labels::chipName(u"esp32c3"_s)}
                                       .join(u", "_s));
        item->setText(FirmwareColumn, tr("ESP-IDF (your folder)"));
        item->setToolTip(FirmwareColumn, tr("Your own ESP-IDF project, built with idf.py. Right-click to forget it."));
        item->setData(NameColumn, Qt::UserRole,
                      QStringList{p.value(u"name"_s).toString(), p.value(u"path"_s).toString(), u"esp-idf"_s}
                          .join(u' ')
                          .toLower());
        m_tree->insertTopLevelItem(at++, item);
    }
}

void LibraryPanel::selectProject(const QString &id)
{
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
        QTreeWidgetItem *item = m_tree->topLevelItem(i);
        if (item->data(NameColumn, IdRole).toString() != id)
            continue;
        if (item->isHidden()) {
            m_filter->clear(); // a filter would hide what the link asked for
            applyFilter();
        }
        m_tree->setCurrentItem(item);
        m_tree->scrollToItem(item);
        return;
    }
    m_pendingSelect = id;
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
