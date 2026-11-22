#include "DiskSpaceDialog.h"

#include "CoreConnection.h"

#include <QDialogButtonBox>
#include <QDir>
#include <QHeaderView>
#include <QJsonObject>
#include <QLabel>
#include <QLocale>
#include <QPushButton>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
constexpr int ProjectRole = Qt::UserRole + 1;
}

DiskSpaceDialog::DiskSpaceDialog(CoreConnection *core, QWidget *parent)
    : QDialog(parent)
    , m_core(core)
    , m_tree(new QTreeWidget(this))
    , m_status(new QLabel(this))
    , m_remove(new QPushButton(QIcon::fromTheme(u"edit-delete"_s), tr("Remove this project's builds"), this))
    , m_removeAll(new QPushButton(QIcon::fromTheme(u"edit-delete"_s), tr("Remove all builds"), this))
{
    setWindowTitle(tr("Disk space"));
    m_tree->setAccessibleName(tr("Disk space"));
    m_tree->setHeaderLabels({tr("What"), tr("Size"), tr("Where")});
    m_tree->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_status->setWordWrap(true);
    m_status->setText(tr("Builds are made again when they are needed. The simulator and the Arduino tools are "
                         "installed from the toolchain setup; the Arduino tools are shared with the Arduino IDE, "
                         "so they are not removed here."));
    m_remove->setEnabled(false);
    connect(m_tree, &QTreeWidget::currentItemChanged, this, [this](QTreeWidgetItem *item) {
        m_remove->setEnabled(item && !item->data(0, ProjectRole).toString().isEmpty());
    });
    connect(m_remove, &QPushButton::clicked, this, [this] {
        if (QTreeWidgetItem *item = m_tree->currentItem())
            m_core->cleanCache(item->data(0, ProjectRole).toString());
    });
    connect(m_removeAll, &QPushButton::clicked, this, [this] { m_core->cleanCache(); });
    connect(m_core, &CoreConnection::cacheUsageListed, this, &DiskSpaceDialog::showUsage);
    connect(m_core, &CoreConnection::cacheCleaned, this, [this](const QString &, const QString &, qint64 freed) {
        m_status->setText(tr("Freed %1.").arg(QLocale().formattedDataSize(freed)));
        refresh();
    });
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, this);
    buttons->addButton(m_remove, QDialogButtonBox::ActionRole);
    buttons->addButton(m_removeAll, QDialogButtonBox::ActionRole);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_tree, 1);
    layout->addWidget(m_status);
    layout->addWidget(buttons);
    resize(640, 420);
}

void DiskSpaceDialog::refresh()
{
    m_core->cacheUsage();
}

QString DiskSpaceDialog::statusText() const
{
    return m_status->text();
}

void DiskSpaceDialog::showUsage(const QJsonArray &categories, const QJsonArray &builds)
{
    m_tree->clear();
    const QLocale loc;
    for (const QJsonValue &v : categories) {
        const QJsonObject c = v.toObject();
        const QString kind = c.value(u"kind"_s).toString();
        auto *item = new QTreeWidgetItem(m_tree);
        item->setText(0, kind == u"builds" ? tr("Builds") : kind == u"simulator" ? tr("Simulator") : tr("Arduino tools"));
        item->setText(1, loc.formattedDataSize(c.value(u"bytes"_s).toInteger()));
        item->setText(2, QDir::toNativeSeparators(c.value(u"path"_s).toString()));
        if (kind != u"builds")
            continue;
        for (const QJsonValue &b : builds) {
            const QJsonObject o = b.toObject();
            auto *project = new QTreeWidgetItem(item);
            project->setText(0, o.value(u"project"_s).toString());
            project->setText(1, loc.formattedDataSize(o.value(u"bytes"_s).toInteger()));
            project->setData(0, ProjectRole, o.value(u"project"_s).toString());
        }
        item->setExpanded(true);
    }
    m_removeAll->setEnabled(!builds.isEmpty());
}
