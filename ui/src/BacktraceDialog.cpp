#include "BacktraceDialog.h"

#include "CoreConnection.h"

#include <QDialogButtonBox>
#include <QFileInfo>
#include <QFontDatabase>
#include <QHeaderView>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTreeWidget>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
constexpr int LocationRole = Qt::UserRole + 1;
}

BacktraceDialog::BacktraceDialog(CoreConnection *core, QWidget *parent)
    : QDialog(parent)
    , m_core(core)
    , m_about(new QLabel(this))
    , m_input(new QPlainTextEdit(this))
    , m_decode(new QPushButton(tr("Decode"), this))
    , m_status(new QLabel(this))
    , m_frames(new QTreeWidget(this))
{
    setWindowTitle(tr("Decode a crash backtrace"));
    m_about->setWordWrap(true);
    m_input->setAccessibleName(tr("Crash output"));
    m_input->setPlaceholderText(tr("Paste what the board printed when it crashed, with its Backtrace: line "
                                   "(or, on an ESP32-C3, its register dump)"));
    m_input->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    m_decode->setDefault(true);
    m_decode->setEnabled(false);
    m_status->setWordWrap(true);
    m_frames->setAccessibleName(tr("Decoded frames"));
    m_frames->setHeaderLabels({tr("Function"), tr("Where"), tr("Address")});
    m_frames->setRootIsDecorated(false);
    m_frames->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_frames->setToolTip(tr("Double-click a line to see it"));
    m_frames->hide();
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, this);
    buttons->addButton(m_decode, QDialogButtonBox::ActionRole);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    connect(m_input, &QPlainTextEdit::textChanged, this,
            [this] { m_decode->setEnabled(!m_project.isEmpty() && !m_input->toPlainText().trimmed().isEmpty()); });
    connect(m_decode, &QPushButton::clicked, this, [this] {
        m_waiting = true;
        m_status->setText(tr("Decoding…"));
        m_core->decodeBacktrace(m_project, m_board, m_input->toPlainText());
    });
    connect(m_core, &CoreConnection::backtraceDecoded, this, [this](const QJsonArray &b) {
        if (std::exchange(m_waiting, false))
            showResult(b);
    });
    connect(m_core, &CoreConnection::requestFailed, this, [this](const QString &method, const QString &message) {
        if (method == u"backtrace.decode" && std::exchange(m_waiting, false)) {
            m_status->setText(tr("Could not decode it: %1").arg(message));
            m_frames->hide();
        }
    });
    connect(m_frames, &QTreeWidget::itemActivated, this, [this](QTreeWidgetItem *item) {
        if (const QJsonObject at = item->data(0, LocationRole).toJsonObject(); !at.isEmpty())
            emit locationActivated(at);
    });

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_about);
    layout->addWidget(m_input, 1);
    layout->addWidget(m_status);
    layout->addWidget(m_frames, 1);
    layout->addWidget(buttons);
    resize(640, 480);
}

void BacktraceDialog::setBuild(const QString &project, const QString &board, const QString &title)
{
    m_project = project;
    m_board = board;
    m_about->setText(tr("Against the build of %1 for %2: decoding needs the same build the board ran.")
                         .arg(title, board));
    m_decode->setEnabled(!m_project.isEmpty() && !m_input->toPlainText().trimmed().isEmpty());
}

QString BacktraceDialog::statusText() const
{
    return m_status->text();
}

void BacktraceDialog::showResult(const QJsonArray &backtraces)
{
    m_frames->clear();
    int count = 0;
    for (const QJsonValue &b : backtraces) {
        for (const QJsonValue &v : b.toObject().value(u"frames"_s).toArray()) {
            const QJsonObject f = v.toObject();
            const QString path = f.value(u"file"_s).toString();
            const QString file = QFileInfo(path).fileName();
            const int line = f.value(u"line"_s).toInt();
            auto *item = new QTreeWidgetItem(m_frames);
            item->setText(0, f.value(u"func"_s).toString(tr("(unknown)")));
            item->setText(1, line > 0 ? tr("%1, line %2").arg(file).arg(line) : file);
            item->setToolTip(1, path);
            item->setText(2, f.value(u"addr"_s).toString());
            if (f.value(u"sketch"_s).toBool()) {
                QFont bold = item->font(0);
                bold.setBold(true);
                item->setFont(0, bold);
                item->setFont(1, bold);
            }
            if (!file.isEmpty() && line > 0)
                item->setData(0, LocationRole, QJsonObject{{u"file"_s, file}, {u"line"_s, line}, {u"path"_s, path}});
            ++count;
        }
    }
    m_frames->setVisible(count > 0);
    m_status->setText(backtraces.isEmpty() ? tr("No backtrace in that text: it needs the Backtrace: line, or on an "
                                                "ESP32-C3 the MEPC and RA of the register dump.")
                                           : tr("Decoded, innermost first; the sketch's own lines are in bold."));
}
