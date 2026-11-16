#include "SetupDialog.h"

#include "CoreConnection.h"

#include <QDialogButtonBox>
#include <QFontDatabase>
#include <QHeaderView>
#include <QIcon>
#include <QJsonObject>
#include <QLabel>
#include <QLocale>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

SetupDialog::SetupDialog(CoreConnection *core, QWidget *parent)
    : QDialog(parent)
    , m_core(core)
    , m_summary(new QLabel(this))
    , m_tree(new QTreeWidget(this))
    , m_log(new QPlainTextEdit(this))
{
    setWindowTitle(tr("Toolchain setup"));
    resize(860, 460);

    auto *intro = new QLabel(tr("Builds need arduino-cli and the ESP32 Arduino core. The simulator needs "
                                "QEMU. Versions are pinned, so builds match what was tested."),
                             this);
    intro->setWordWrap(true);
    m_summary->setWordWrap(true);

    m_tree->setColumnCount(ColumnCount);
    m_tree->setHeaderLabels({tr("Component"), tr("Needed for"), tr("Status"), tr("Version"),
                             tr("Download"), QString()});
    m_tree->setRootIsDecorated(false);
    m_tree->setSelectionMode(QAbstractItemView::NoSelection);
    m_tree->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_tree->header()->setStretchLastSection(true);

    m_log->setReadOnly(true);
    m_log->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    m_log->setMaximumBlockCount(5000);
    m_log->setPlaceholderText(tr("Install progress appears here."));

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, this);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(intro);
    layout->addWidget(m_tree, 2);
    layout->addWidget(m_summary);
    layout->addWidget(m_log, 1);
    layout->addWidget(buttons);

    connect(m_core, &CoreConnection::toolchainStatus, this, &SetupDialog::showStatus);
    connect(m_core, &CoreConnection::toolchainProgress, m_log, &QPlainTextEdit::appendPlainText);
    connect(m_core, &CoreConnection::toolchainInstallFinished, this, [this](bool ok, const QString &error) {
        m_log->appendPlainText(ok ? tr("Done.") : tr("Install failed: %1").arg(error));
        m_core->requestToolchainStatus();
    });
}

QString SetupDialog::logText() const
{
    return m_log->toPlainText();
}

void SetupDialog::install(const QString &component, const QString &name)
{
    m_log->appendPlainText(tr("Installing %1…").arg(name));
    m_core->installComponent(component);
    // Disable every install button until the status comes back.
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i)
        if (QWidget *w = m_tree->itemWidget(m_tree->topLevelItem(i), ActionColumn))
            w->setEnabled(false);
}

void SetupDialog::showStatus(const QJsonArray &components)
{
    m_tree->clear();
    int missingForBuild = 0;
    for (const QJsonValue &v : components) {
        const QJsonObject c = v.toObject();
        const QString id = c.value(u"id"_s).toString();
        const QString name = c.value(u"name"_s).toString();
        const QString state = c.value(u"state"_s).toString();
        const bool installed = state == u"installed";
        const bool forBuild = c.value(u"neededFor"_s).toString() == u"build";
        if (forBuild && !installed)
            ++missingForBuild;

        auto *item = new QTreeWidgetItem(m_tree);
        item->setText(NameColumn, name);
        item->setText(NeededForColumn, forBuild ? tr("Building") : tr("Simulator"));
        item->setText(StateColumn, installed                  ? tr("Installed")
                                   : state == u"other-version" ? tr("Different version")
                                   : state == u"broken"        ? tr("Not working")
                                                               : tr("Missing"));
        item->setIcon(StateColumn, QIcon::fromTheme(installed ? u"dialog-ok-apply"_s : u"dialog-warning"_s));

        const QString version = c.value(u"version"_s).toString();
        const QString wanted = c.value(u"wanted"_s).toString();
        item->setText(VersionColumn, !wanted.isEmpty() && version != wanted
                                         ? (version.isEmpty() ? tr("needs %1").arg(wanted)
                                                              : tr("%1, needs %2").arg(version, wanted))
                                         : version);
        if (c.contains(u"downloadBytes"_s))
            item->setText(SizeColumn, QLocale().formattedDataSize(c.value(u"downloadBytes"_s).toInteger()));
        else if (!installed && c.value(u"installable"_s).toBool())
            item->setText(SizeColumn, tr("unknown"));

        const QString hint = c.value(u"hint"_s).toString();
        if (!hint.isEmpty()) {
            for (int col = 0; col < ColumnCount; ++col)
                item->setToolTip(col, hint);
        }
        if (!installed && c.value(u"installable"_s).toBool()) {
            auto *button = new QPushButton(QIcon::fromTheme(u"download"_s), tr("Install"), m_tree);
            button->setEnabled(!m_core->isInstalling());
            connect(button, &QPushButton::clicked, this, [this, id, name] { install(id, name); });
            m_tree->setItemWidget(item, ActionColumn, button);
        } else if (!installed) {
            // Not something the app can install (e.g. needs root): say what to do.
            item->setText(ActionColumn, hint);
        }
    }
    m_summary->setText(missingForBuild == 0
                           ? tr("Everything needed for builds is installed.")
                           : tr("Missing for builds: %1. Hover over a row for details.").arg(missingForBuild));
}
