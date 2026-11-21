#include "LibraryPanel.h"

#include "CoreConnection.h"
#include "Labels.h"

#include <algorithm>

#include <QCheckBox>
#include <QComboBox>
#include <QHash>
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
#include <QStyle>
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

// How wide column `c` needs to be for its header and every row's text.
int contentWidth(const QTreeWidget *tree, int c)
{
    const int padding = 2 * tree->style()->pixelMetric(QStyle::PM_FocusFrameHMargin, nullptr, tree) + 8;
    int w = tree->header()->sectionSizeHint(c);
    for (int i = 0; i < tree->topLevelItemCount(); ++i)
        w = std::max(w, tree->fontMetrics().horizontalAdvance(tree->topLevelItem(i)->text(c)) + padding);
    return w;
}

} // namespace

LibraryPanel::LibraryPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_filter(new QLineEdit(this))
    , m_status(new QLabel(this))
    , m_refresh(new QToolButton(this))
    , m_open(new QToolButton(this))
    , m_tree(new QTreeWidget(this))
    , m_simOnly(new QCheckBox(tr("Runs in the simulator"), this))
    , m_fitsBoard(new QCheckBox(tr("Fits my board"), this))
    , m_starredOnly(new QCheckBox(tr("Starred"), this))
    , m_builtOnly(new QCheckBox(tr("Built"), this))
    , m_tag(new QComboBox(this))
{
    m_builtOnly->setToolTip(tr("Only projects built on this computer, ready to run or flash"));
    connect(m_builtOnly, &QCheckBox::toggled, this, &LibraryPanel::applyFilter);
    m_tag->addItem(tr("All topics"), QString());
    m_tag->setAccessibleName(tr("Topic"));
    m_tag->setToolTip(tr("Only projects about one topic, such as a display or Wi-Fi"));
    m_tag->setSizeAdjustPolicy(QComboBox::AdjustToContents);
    connect(m_tag, &QComboBox::currentIndexChanged, this, &LibraryPanel::applyFilter);
    m_filter->setPlaceholderText(tr("Search projects"));
    m_filter->setMinimumWidth(m_filter->fontMetrics().averageCharWidth() * 16);
    m_filter->setClearButtonEnabled(true);
    m_filter->setAccessibleName(tr("Search projects"));
    m_tree->setAccessibleName(tr("Projects"));
    connect(m_filter, &QLineEdit::textChanged, this, &LibraryPanel::applyFilter);

    m_refresh->setIcon(QIcon::fromTheme(u"view-refresh"_s));
    m_refresh->setText(tr("Refresh"));
    m_refresh->setToolTip(tr("Download the project library from esp32bench.dev again"));
    m_refresh->setToolButtonStyle(Qt::ToolButtonIconOnly);
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
    // Every column can be dragged wider or narrower; the last one shown takes what is
    // left. The name column starts as wide as the longest name (fitColumns).
    m_tree->header()->setSectionResizeMode(QHeaderView::Interactive);
    m_tree->header()->setStretchLastSection(true);
    m_tree->header()->setMinimumSectionSize(m_tree->fontMetrics().averageCharWidth() * 4);
    connect(m_tree->header(), &QHeaderView::sectionResized, this, [this](int column) {
        // The last column shown follows the panel's width; only the others are the user's.
        QHeaderView *h = m_tree->header();
        const int last = h->logicalIndex(h->count() - 1 - h->hiddenSectionCount());
        if (!m_fitting && column != last && !h->isSectionHidden(column))
            m_userSized = true;
    });
    // The name, level and chips are what a choice needs; time and firmware are in the
    // project view. Right-click the header to show them again.
    m_tree->setColumnHidden(TimeColumn, true);
    m_tree->setColumnHidden(FirmwareColumn, true);
    m_tree->header()->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(m_tree->header(), &QHeaderView::customContextMenuRequested, this, [this](const QPoint &pos) {
        QMenu menu(this);
        for (int c = LevelColumn; c < ColumnCount; ++c) {
            QAction *a = menu.addAction(m_tree->headerItem()->text(c));
            a->setCheckable(true);
            a->setChecked(!m_tree->isColumnHidden(c));
            connect(a, &QAction::toggled, this, [this, c](bool on) { m_tree->setColumnHidden(c, !on); });
        }
        menu.exec(m_tree->header()->viewport()->mapToGlobal(pos));
    });
    m_defaultColumns = m_tree->header()->saveState();
    m_userSized = false; // hiding columns above resized them; that was not the user
    // Double-click or Enter runs the project (MainWindow builds it first if needed).
    connect(m_tree, &QTreeWidget::itemActivated, this, [this](QTreeWidgetItem *item) {
        if (item)
            emit projectActivated(item->data(NameColumn, IdRole).toString());
    });
    connect(m_tree, &QTreeWidget::currentItemChanged, this, [this](QTreeWidgetItem *item) {
        if (item)
            emit projectSelected(item->data(NameColumn, IdRole).toString());
    });
    m_tree->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(m_tree, &QTreeWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        QTreeWidgetItem *item = m_tree->itemAt(pos);
        if (!item)
            return;
        const QString id = item->data(NameColumn, IdRole).toString();
        QMenu menu(this);
        if (item->data(NameColumn, LocalRole).toBool()) {
            const QString name = item->text(NameColumn);
            const QString path = item->data(NameColumn, PathRole).toString();
            menu.addAction(QIcon::fromTheme(u"list-remove"_s), tr("Forget this project"), this, [this, id, name, path] {
                m_core->removeLocal(id);
                emit projectForgotten(name, path);
            });
        } else {
            const bool starred = item->data(NameColumn, StarredRole).toBool();
            menu.addAction(QIcon::fromTheme(u"starred"_s),
                           starred ? tr("Remove the star") : tr("Star this project"), this,
                           [this, id, starred] { m_core->starProject(id, !starred); });
        }
        menu.exec(m_tree->viewport()->mapToGlobal(pos));
    });

    m_simOnly->setToolTip(tr("Only projects that run in the simulator, with no board needed"));
    connect(m_simOnly, &QCheckBox::toggled, this, &LibraryPanel::applyFilter);
    m_fitsBoard->setEnabled(false);
    m_fitsBoard->setToolTip(tr("Connect a board to show only the projects for its chip"));
    connect(m_fitsBoard, &QCheckBox::toggled, this, &LibraryPanel::applyFilter);
    m_starredOnly->setToolTip(tr("Only the projects you starred (right-click a project to star it)"));
    connect(m_starredOnly, &QCheckBox::toggled, this, &LibraryPanel::applyFilter);
    // The chips of connected boards, from ports that say which chip they have.
    connect(m_core, &CoreConnection::devicesListed, this, [this](const QJsonArray &ports) {
        QStringList chips;
        for (const QJsonValue &v : ports) {
            const QJsonArray c = v.toObject().value(u"chips"_s).toArray();
            if (c.size() == 1 && !chips.contains(c.at(0).toString()))
                chips << c.at(0).toString();
        }
        m_connectedChips = chips;
        QStringList names;
        for (const QString &c : std::as_const(chips))
            names << Labels::chipName(c);
        m_fitsBoard->setEnabled(!chips.isEmpty());
        m_fitsBoard->setToolTip(chips.isEmpty()
                                    ? tr("Connect a board to show only the projects for its chip")
                                    : tr("Only projects with a build for the connected %1").arg(names.join(u", "_s)));
        applyFilter();
    });

    auto *top = new QHBoxLayout;
    top->addWidget(m_filter, 1);
    top->addWidget(m_refresh);
    top->addWidget(m_open);
    auto *filters = new QHBoxLayout;
    filters->addWidget(m_simOnly);
    filters->addWidget(m_fitsBoard);
    filters->addWidget(m_starredOnly);
    filters->addWidget(m_builtOnly);
    filters->addWidget(m_tag);
    filters->addStretch(1);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addLayout(filters);
    layout->addWidget(m_status);
    layout->addWidget(m_tree, 1);

    connect(m_core, &CoreConnection::libraryListed, this, &LibraryPanel::showLibrary);
    // A build changes what is built: list again, for the bold names and the filter.
    connect(m_core, &CoreConnection::buildFinished, this, [this](bool ok) {
        if (ok)
            m_core->listLibrary();
    });
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

    // The project picked stays picked when the list is redrawn (after a build, say),
    // without loading it again.
    const QString picked = m_tree->currentItem() ? m_tree->currentItem()->data(NameColumn, IdRole).toString()
                                                  : QString();
    const bool wasBlocked = m_tree->blockSignals(true);
    m_tree->setSortingEnabled(false); // insert in the site's order
    m_tree->clear();
    const QJsonArray projects = library.value(u"projects"_s).toArray();
    // Starred projects first, each group in the site's order.
    QList<QJsonObject> ordered;
    for (const QJsonValue &v : projects) {
        if (v.toObject().value(u"starred"_s).toBool())
            ordered << v.toObject();
    }
    for (const QJsonValue &v : projects) {
        if (!v.toObject().value(u"starred"_s).toBool())
            ordered << v.toObject();
    }
    for (const QJsonObject &p : std::as_const(ordered)) {
        auto *item = new ProjectItem(m_tree);
        QStringList tags;
        for (const QJsonValue &t : p.value(u"tags"_s).toArray())
            tags << t.toString();
        item->setData(NameColumn, TagsRole, tags);
        QStringList built;
        for (const QJsonValue &b : p.value(u"built"_s).toArray())
            built << b.toString();
        item->setData(NameColumn, BuiltRole, built);
        if (!built.isEmpty()) {
            // Built ones stand out: ready to run or flash.
            QFont f = item->font(NameColumn);
            f.setBold(true);
            item->setFont(NameColumn, f);
        }
        const bool starred = p.value(u"starred"_s).toBool();
        item->setData(NameColumn, StarredRole, starred);
        // A star before a starred project's name: a symbol, not words to translate.
        item->setText(NameColumn, starred ? u"★ %1"_s.arg(p.value(u"name"_s).toString())
                                          : p.value(u"name"_s).toString());
        item->setToolTip(NameColumn, p.value(u"summary"_s).toString());
        item->setData(NameColumn, UrlRole, p.value(u"url"_s).toString());
        item->setData(NameColumn, IdRole, p.value(u"id"_s).toString());

        const QString level = p.value(u"level"_s).toString();
        item->setText(LevelColumn, level == u"Starter" ? tr("Starter")
                                   : level == u"Intermediate" ? tr("Intermediate")
                              : level == u"Advanced" ? tr("Advanced")
                                                              : level);
        const int minutes = p.value(u"minutes"_s).toInt();
        item->setText(TimeColumn, tr("%1 min").arg(minutes));
        item->setData(TimeColumn, Qt::UserRole, minutes);

        QStringList chips;
        QStringList chipIds;
        for (const QJsonValue &c : p.value(u"chips"_s).toArray()) {
            chips << Labels::chipName(c.toString());
            chipIds << c.toString();
        }
        item->setText(ChipsColumn, chips.join(u", "_s));
        item->setData(NameColumn, ChipsRole, chipIds);
        const bool simulated = p.value(u"simulator"_s).toBool();
        item->setData(NameColumn, SimulatorRole, simulated);
        if (simulated) {
            item->setIcon(NameColumn, QIcon::fromTheme(u"media-playback-start"_s));
            item->setToolTip(NameColumn,
                             tr("%1\n\nRuns in the simulator.").arg(p.value(u"summary"_s).toString()));
        }
        if (!built.isEmpty())
            item->setToolTip(NameColumn, tr("%1\n\nBuilt on this computer for: %2.")
                                             .arg(item->toolTip(NameColumn), built.join(u", "_s)));

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
    fitColumns();
    fillTopics();

    const qint64 fetched = library.value(u"fetchedAt"_s).toInteger();
    const QString when = fetched > 0
        ? QLocale().toString(QDateTime::fromSecsSinceEpoch(fetched), QLocale::ShortFormat)
        : tr("unknown");
    setStatus(tr("Projects: %1. Downloaded %2.").arg(projects.size()).arg(when));
    applyFilter();
    for (int i = 0; !picked.isEmpty() && i < m_tree->topLevelItemCount(); ++i) {
        if (m_tree->topLevelItem(i)->data(NameColumn, IdRole).toString() == picked)
            m_tree->setCurrentItem(m_tree->topLevelItem(i));
    }
    m_tree->blockSignals(wasBlocked); // a project asked for meanwhile (a link) is loaded
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
    fitColumns();
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
        item->setData(NameColumn, PathRole, p.value(u"path"_s).toString());
        item->setData(NameColumn, IdRole, p.value(u"id"_s).toString());
        item->setData(NameColumn, LocalRole, true);
        // Built for the chip chosen, so any of the three the simulator runs.
        item->setData(NameColumn, SimulatorRole, true);
        item->setData(NameColumn, ChipsRole, QStringList{u"esp32"_s, u"esp32s3"_s, u"esp32c3"_s});
        item->setIcon(NameColumn, QIcon::fromTheme(u"media-playback-start"_s));
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

void LibraryPanel::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    fitColumns(); // the name gets the room a wider panel gives, until the user sizes it
}

void LibraryPanel::fitColumns()
{
    if (m_userSized)
        return;
    m_fitting = true;
    for (int c = LevelColumn; c < ColumnCount; ++c)
        m_tree->resizeColumnToContents(c);
    // The other columns' room, by what they hold (the last one's width includes its
    // stretch); the name gets the rest.
    int others = 0;
    for (int c = LevelColumn; c < ColumnCount; ++c) {
        if (!m_tree->isColumnHidden(c))
            others += contentWidth(m_tree, c);
    }
    m_tree->resizeColumnToContents(NameColumn);
    // It takes the room the others leave, so a long name is cut only when the panel
    // is too narrow for it; then dragging it wider scrolls the others into view.
    const int room = m_tree->viewport()->width() - others;
    if (room > m_tree->header()->minimumSectionSize() * 4)
        m_tree->setColumnWidth(NameColumn, room);
    m_fitting = false;
}

QByteArray LibraryPanel::saveColumns() const
{
    return m_userSized ? m_tree->header()->saveState() : QByteArray();
}

void LibraryPanel::restoreColumns(const QByteArray &state)
{
    if (state.isEmpty())
        return;
    m_fitting = true;
    m_userSized = m_tree->header()->restoreState(state);
    m_fitting = false;
}

void LibraryPanel::resetColumns()
{
    m_fitting = true;
    m_tree->header()->restoreState(m_defaultColumns);
    m_fitting = false;
    m_userSized = false;
    fitColumns();
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
        bool match = std::all_of(terms.begin(), terms.end(), [&](const QString &t) { return text.contains(t); });
        if (m_simOnly->isChecked() && !item->data(NameColumn, SimulatorRole).toBool())
            match = false;
        if (m_starredOnly->isChecked() && !item->data(NameColumn, StarredRole).toBool())
            match = false;
        if (m_builtOnly->isChecked() && item->data(NameColumn, BuiltRole).toStringList().isEmpty())
            match = false;
        if (const QString topic = m_tag->currentData().toString();
            !topic.isEmpty() && !item->data(NameColumn, TagsRole).toStringList().contains(topic))
            match = false;
        if (m_fitsBoard->isChecked() && m_fitsBoard->isEnabled()) {
            const QStringList chips = item->data(NameColumn, ChipsRole).toStringList();
            match = match && std::any_of(chips.begin(), chips.end(),
                                         [&](const QString &c) { return m_connectedChips.contains(c); });
        }
        item->setHidden(!match);
    }
}

QList<std::pair<QString, QString>> LibraryPanel::projects() const
{
    QList<std::pair<QString, QString>> out;
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
        const QTreeWidgetItem *item = m_tree->topLevelItem(i);
        QString name = item->text(NameColumn);
        if (item->data(NameColumn, StarredRole).toBool())
            name = name.mid(2); // without the star
        out.append({name, item->data(NameColumn, IdRole).toString()});
    }
    return out;
}

void LibraryPanel::fillTopics()
{
    // The tags, most used first (with how many projects have each); the choice stays.
    QHash<QString, int> count;
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
        for (const QString &t : m_tree->topLevelItem(i)->data(NameColumn, TagsRole).toStringList())
            ++count[t];
    }
    QStringList tags = count.keys();
    std::sort(tags.begin(), tags.end(), [&](const QString &a, const QString &b) {
        return count.value(a) != count.value(b) ? count.value(a) > count.value(b) : a < b;
    });
    const QString current = m_tag->currentData().toString();
    const QSignalBlocker block(m_tag);
    while (m_tag->count() > 1)
        m_tag->removeItem(1);
    for (const QString &t : std::as_const(tags))
        m_tag->addItem(tr("%1 (%2)").arg(t).arg(count.value(t)), t);
    const int i = m_tag->findData(current);
    m_tag->setCurrentIndex(i < 0 ? 0 : i);
    m_tag->setEnabled(m_tag->count() > 1);
}
