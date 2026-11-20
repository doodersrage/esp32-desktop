#include "DevicesPanel.h"

#include "CoreConnection.h"
#include "Labels.h"

#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QJsonObject>
#include <QLabel>
#include <QMenu>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

QString DevicesPanel::bridgeName(const QJsonValue &bridge)
{
    const QString b = bridge.toString();
    if (b == u"cp210x") return tr("CP210x USB to UART");
    if (b == u"ch340") return tr("CH340 USB to UART");
    if (b == u"ch9102") return tr("CH9102 USB to UART");
    if (b == u"ftdi") return tr("FTDI USB to UART");
    if (b == u"pl2303") return tr("PL2303 USB to UART");
    if (b == u"usb-serial-jtag") return tr("Built-in USB Serial/JTAG");
    if (b == u"usb-cdc") return tr("Built-in USB (firmware CDC)");
    return tr("Unknown");
}

QString DevicesPanel::accessHint()
{
    return tr("You cannot open this port. Install the esp32bench package, which adds "
              "a udev rule, or add yourself to the group that owns serial ports: uucp on "
              "Arch, dialout on Debian, Ubuntu and Fedora.");
}

DevicesPanel::DevicesPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_tree(new QTreeWidget(this))
    , m_status(new QLabel(this))
    , m_refresh(new QToolButton(this))
{
    m_refresh->setIcon(QIcon::fromTheme(u"view-refresh"_s));
    m_refresh->setText(tr("Refresh"));
    m_refresh->setToolTip(tr("Look for connected boards again"));
    m_refresh->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    connect(m_refresh, &QToolButton::clicked, this, &DevicesPanel::refresh);

    m_status->setWordWrap(true);

    m_tree->setColumnCount(ColumnCount);
    m_tree->setHeaderLabels({tr("Port"), tr("Possible chips"), tr("USB device"), tr("USB ID")});
    m_tree->setRootIsDecorated(false);
    m_tree->setUniformRowHeights(true);
    // Short columns fit their text; the long USB device name takes what is left and
    // is elided, so a narrow dock needs no horizontal scrollbar.
    m_tree->header()->setStretchLastSection(false);
    m_tree->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_tree->header()->setSectionResizeMode(BridgeColumn, QHeaderView::Stretch);
    m_tree->setTextElideMode(Qt::ElideRight);
    m_tree->setColumnHidden(UsbIdColumn, true); // in each row's tooltip instead
    m_tree->setToolTip(tr("Double-click a port to open it in the serial monitor; right-click for more"));
    m_tree->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(m_tree, &QTreeWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        QTreeWidgetItem *item = m_tree->itemAt(pos);
        if (!item)
            return;
        const QString path = item->text(PortColumn);
        QMenu menu(this);
        menu.addAction(QIcon::fromTheme(u"utilities-terminal"_s), tr("Open in serial monitor"), this,
                       [this, path] { emit portActivated(path); });
        menu.addAction(QIcon::fromTheme(u"document-properties"_s), tr("Inspect board…"), this,
                       [this, path] { emit inspectRequested(path); });
        menu.addSeparator();
        menu.addAction(QIcon::fromTheme(u"edit-delete"_s), tr("Erase flash…"), this,
                       [this, path] { emit eraseRequested(path); });
        menu.exec(m_tree->viewport()->mapToGlobal(pos));
    });
    connect(m_tree, &QTreeWidget::itemActivated, this, [this](QTreeWidgetItem *item) {
        emit portActivated(item->text(PortColumn));
    });

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_refresh);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_tree, 1);

    connect(m_core, &CoreConnection::devicesListed, this, &DevicesPanel::showPorts);
    connect(m_core, &CoreConnection::requestFailed, this,
            [this](const QString &method, const QString &message) {
                if (method == u"device.list")
                    showMessage(tr("Could not list devices: %1").arg(message));
            });
    connect(m_core, &CoreConnection::stateChanged, this, [this](CoreConnection::State s) {
        updateEnabled();
        // Subscribe once per connection; later changes arrive by themselves.
        if (s == CoreConnection::State::Connected)
            m_core->watchDevices();
    });
    updateEnabled();
}

QString DevicesPanel::statusText() const
{
    return m_status->text();
}

void DevicesPanel::refresh()
{
    if (m_core->state() != CoreConnection::State::Connected)
        return;
    m_core->listDevices();
}

void DevicesPanel::updateEnabled()
{
    const bool up = m_core->state() == CoreConnection::State::Connected;
    m_refresh->setEnabled(up);
    if (!up) {
        m_tree->clear();
        showMessage(tr("Not connected to the core."));
    }
}

void DevicesPanel::showMessage(const QString &text)
{
    m_status->setText(text);
}

void DevicesPanel::showPorts(const QJsonArray &ports)
{
    m_tree->clear();
    int denied = 0;
    for (const QJsonValue &v : ports) {
        const QJsonObject p = v.toObject();
        auto *item = new QTreeWidgetItem(m_tree);
        item->setText(PortColumn, p.value(u"path"_s).toString());

        QStringList chips;
        for (const QJsonValue &c : p.value(u"chips"_s).toArray())
            chips << Labels::chipName(c.toString());
        item->setText(ChipsColumn, chips.isEmpty() ? tr("Any") : chips.join(u", "_s));
        if (chips.isEmpty())
            item->setToolTip(ChipsColumn, tr("A USB to UART bridge does not tell which chip is behind it."));

        item->setText(BridgeColumn, bridgeName(p.value(u"bridge"_s)));
        const QJsonObject usb = p.value(u"usb"_s).toObject();
        if (!usb.isEmpty()) {
            const QString id = u"%1:%2"_s
                                   .arg(usb.value(u"vid"_s).toInt(), 4, 16, QLatin1Char('0'))
                                   .arg(usb.value(u"pid"_s).toInt(), 4, 16, QLatin1Char('0'));
            item->setText(UsbIdColumn, id);
            const QString product = usb.value(u"product"_s).toString();
            item->setToolTip(BridgeColumn, product.isEmpty() ? tr("USB ID %1").arg(id)
                                                             : tr("%1, USB ID %2").arg(product, id));
        }

        if (p.value(u"access"_s).toString() == u"denied") {
            ++denied;
            item->setIcon(PortColumn, QIcon::fromTheme(u"dialog-warning"_s));
            item->setToolTip(PortColumn, accessHint());
        } else {
            item->setIcon(PortColumn, QIcon::fromTheme(u"network-serial"_s,
                                                        QIcon::fromTheme(u"preferences-system-network"_s)));
        }
    }

    if (ports.isEmpty())
        showMessage(tr("No boards found. Plug one in and press Refresh."));
    else if (denied > 0)
        showMessage(tr("Boards found: %1. Ports you cannot open: %2.").arg(ports.size()).arg(denied));
    else
        showMessage(tr("Boards found: %1.").arg(ports.size()));
}
