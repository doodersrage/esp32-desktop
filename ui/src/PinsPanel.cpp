#include "PinsPanel.h"

#include "CoreConnection.h"

#include <QHeaderView>
#include <QIcon>
#include <QJsonObject>
#include <QLabel>
#include <QPushButton>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
constexpr int GpioRole = Qt::UserRole + 1;
}

PinsPanel::PinsPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(this))
    , m_tree(new QTreeWidget(this))
{
    m_tree->setAccessibleName(tr("Pins"));
    m_status->setWordWrap(true);
    m_tree->setColumnCount(ColumnCount);
    m_tree->setHeaderLabels({tr("GPIO"), tr("Direction"), tr("Level"), tr("Drive")});
    m_tree->setRootIsDecorated(false);
    m_tree->setUniformRowHeights(true);
    m_tree->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_tree->setToolTip(tr("The GPIOs this project uses. Outputs show what the firmware drives; "
                          "the switch drives a pin as an input, like a button."));

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_status);
    layout->addWidget(m_tree, 1);

    connect(core, &CoreConnection::simStarted, this, &PinsPanel::started);
    connect(core, &CoreConnection::simPinsChanged, this, &PinsPanel::update);
    connect(core, &CoreConnection::simStopped, this, [this] {
        if (m_tree->topLevelItemCount() > 0)
            m_status->setText(tr("The simulator has stopped."));
        for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
            if (QWidget *w = m_tree->itemWidget(m_tree->topLevelItem(i), DriveColumn))
                w->setEnabled(false);
        }
    });
    clearRows(tr("Run a project in the simulator to see its pins."));
}

QString PinsPanel::statusText() const
{
    return m_status->text();
}

QTreeWidgetItem *PinsPanel::row(int gpio) const
{
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
        if (m_tree->topLevelItem(i)->data(GpioColumn, GpioRole).toInt() == gpio)
            return m_tree->topLevelItem(i);
    }
    return nullptr;
}

void PinsPanel::clearRows(const QString &status)
{
    m_tree->clear();
    m_status->setText(status);
}

void PinsPanel::started()
{
    const QList<int> pins = m_core->simPins();
    if (pins.isEmpty()) {
        clearRows(tr("No pin access in this simulation. Pins are bridged once lcgamboa's QEMU is installed "
                     "(Tools > Toolchain setup), for projects that use GPIOs."));
        return;
    }
    clearRows(tr("GPIOs this project uses. Outputs are reported for GPIO 0 to 31."));
    for (int gpio : pins) {
        auto *item = new QTreeWidgetItem(m_tree);
        const QString label = m_labels.value(gpio);
        item->setText(GpioColumn, label.isEmpty() ? tr("GPIO %1").arg(gpio) : tr("GPIO %1 · %2").arg(gpio).arg(label));
        item->setData(GpioColumn, GpioRole, gpio);
        item->setText(DirectionColumn, tr("not set"));
        item->setText(LevelColumn, tr("unknown"));
        auto *drive = new QPushButton(tr("Low"), m_tree);
        drive->setCheckable(true);
        drive->setToolTip(tr("Drive GPIO %1 high or low as an input").arg(gpio));
        connect(drive, &QPushButton::toggled, this, [this, gpio, drive](bool high) {
            drive->setText(high ? tr("High") : tr("Low"));
            m_core->setSimPin(gpio, high);
        });
        m_tree->setItemWidget(item, DriveColumn, drive);
    }
}

void PinsPanel::update(const QJsonArray &pins)
{
    for (const QJsonValue &v : pins) {
        const QJsonObject p = v.toObject();
        QTreeWidgetItem *item = row(p.value(u"gpio"_s).toInt());
        if (!item)
            continue;
        if (p.contains(u"output"_s)) {
            const bool output = p.value(u"output"_s).toBool();
            item->setText(DirectionColumn, output ? tr("output") : tr("input"));
        }
        if (p.contains(u"level"_s)) {
            const bool high = p.value(u"level"_s).toBool();
            item->setText(LevelColumn, high ? tr("high") : tr("low"));
            // Theme icons, so the indicator follows the Plasma colour scheme.
            item->setIcon(LevelColumn, QIcon::fromTheme(high ? u"state-ok"_s : u"state-offline"_s));
        }
    }
}

void PinsPanel::setLabels(const QHash<int, QString> &labels)
{
    m_labels = labels;
    for (int i = 0; i < m_tree->topLevelItemCount(); ++i) {
        QTreeWidgetItem *item = m_tree->topLevelItem(i);
        const int gpio = item->data(GpioColumn, GpioRole).toInt();
        const QString label = m_labels.value(gpio);
        item->setText(GpioColumn, label.isEmpty() ? tr("GPIO %1").arg(gpio) : tr("GPIO %1 · %2").arg(gpio).arg(label));
    }
}
