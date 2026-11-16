#include "BoardInfoDialog.h"

#include "Labels.h"

#include <QDialogButtonBox>
#include <QHeaderView>
#include <QJsonArray>
#include <QLabel>
#include <QLocale>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

BoardInfoDialog::BoardInfoDialog(QWidget *parent)
    : QDialog(parent)
    , m_summary(new QLabel(this))
    , m_partitions(new QTreeWidget(this))
{
    setWindowTitle(tr("Board"));
    resize(620, 440);
    m_summary->setTextFormat(Qt::RichText);
    m_summary->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_partitions->setHeaderLabels({tr("Partition"), tr("Type"), tr("Subtype"), tr("Offset"), tr("Size")});
    m_partitions->setRootIsDecorated(false);
    m_partitions->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, this);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_summary);
    layout->addWidget(new QLabel(tr("Partition table (at 0x8000):"), this));
    layout->addWidget(m_partitions, 1);
    layout->addWidget(buttons);
}

QString BoardInfoDialog::summaryText() const
{
    return m_summary->text();
}

void BoardInfoDialog::showResult(const QString &port, const QJsonObject &result)
{
    const QJsonObject info = result.value(u"info"_s).toObject();
    setWindowTitle(tr("Board on %1").arg(port));
    QStringList features;
    for (const QJsonValue &v : info.value(u"features"_s).toArray())
        features << v.toString();
    auto row = [](const QString &k, const QString &v) {
        return u"<tr><td><b>%1</b>&nbsp;&nbsp;</td><td>%2</td></tr>"_s.arg(k.toHtmlEscaped(), v.toHtmlEscaped());
    };
    QString html = u"<table>"_s;
    html += row(tr("Chip"), tr("%1 %2").arg(Labels::chipName(info.value(u"chip"_s).toString()),
                                            info.value(u"revision"_s).toString()));
    html += row(tr("Crystal"), tr("%1 MHz").arg(info.value(u"crystalMhz"_s).toInt()));
    html += row(tr("Flash"), QLocale().formattedDataSize(info.value(u"flashBytes"_s).toInteger()));
    html += row(tr("Features"), features.join(u", "_s));
    html += row(tr("MAC"), info.value(u"mac"_s).toString());
    const QJsonObject sec = info.value(u"security"_s).toObject();
    if (sec.isEmpty()) {
        html += row(tr("Security"), tr("not reported by this chip's ROM"));
    } else {
        auto onOff = [](bool b) { return b ? tr("on") : tr("off"); };
        html += row(tr("Secure boot"), onOff(sec.value(u"secureBoot"_s).toBool()));
        html += row(tr("Flash encryption"), onOff(sec.value(u"flashEncryption"_s).toBool()));
        html += row(tr("Secure download"), onOff(sec.value(u"secureDownload"_s).toBool()));
    }
    html += u"</table>"_s;
    m_summary->setText(html);

    m_partitions->clear();
    const QJsonArray parts = result.value(u"partitions"_s).toArray();
    for (const QJsonValue &v : parts) {
        const QJsonObject p = v.toObject();
        auto *item = new QTreeWidgetItem(m_partitions);
        item->setText(0, p.value(u"label"_s).toString());
        item->setText(1, p.value(u"type"_s).toString());
        item->setText(2, p.value(u"subtype"_s).toString());
        item->setText(3, u"0x%1"_s.arg(p.value(u"offset"_s).toInteger(), 6, 16, u'0'));
        item->setText(4, QLocale().formattedDataSize(p.value(u"size"_s).toInteger()));
    }
    if (parts.isEmpty())
        new QTreeWidgetItem(m_partitions, {tr("No partition table found (erased flash?)")});
}
