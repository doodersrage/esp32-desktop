#include "BuildPanel.h"

#include "CoreConnection.h"

#include <QComboBox>
#include <QFontDatabase>
#include <QJsonArray>
#include <QProgressBar>
#include <QHBoxLayout>
#include <QIcon>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {
// Keep the log bounded; arduino-cli can print a lot with many libraries.
constexpr int MaxLogLines = 20000;
}

BuildPanel::BuildPanel(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_status(new QLabel(tr("No build yet."), this))
    , m_cancel(new QPushButton(QIcon::fromTheme(u"process-stop"_s), tr("Cancel"), this))
    , m_log(new QPlainTextEdit(this))
    , m_port(new QComboBox(this))
    , m_flash(new QPushButton(QIcon::fromTheme(u"media-flash"_s, QIcon::fromTheme(u"document-send"_s)),
                              tr("Flash to board"), this))
    , m_progress(new QProgressBar(this))
{
    m_port->setToolTip(tr("Board to flash"));
    m_port->setMinimumContentsLength(12);
    m_flash->setToolTip(tr("Write the last successful build to the selected board"));
    connect(m_flash, &QPushButton::clicked, this, [this] {
        emit flashRequested(m_port->currentData().toString(), m_builtProject, m_builtBoard);
    });
    m_progress->setVisible(false);
    m_progress->setTextVisible(true);
    m_status->setWordWrap(true);
    m_status->setTextInteractionFlags(Qt::TextSelectableByMouse);
    m_cancel->setEnabled(false);
    connect(m_cancel, &QPushButton::clicked, m_core, &CoreConnection::cancelBuild);

    m_log->setReadOnly(true);
    m_log->setLineWrapMode(QPlainTextEdit::NoWrap);
    m_log->setMaximumBlockCount(MaxLogLines);
    m_log->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));

    auto *top = new QHBoxLayout;
    top->addWidget(m_status, 1);
    top->addWidget(m_cancel);
    top->addWidget(m_port);
    top->addWidget(m_flash);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_progress);
    layout->addWidget(m_log, 1);

    connect(m_core, &CoreConnection::buildStarted, this, &BuildPanel::started);
    connect(m_core, &CoreConnection::buildLog, this, &BuildPanel::appendLine);
    connect(m_core, &CoreConnection::buildFinished, this, &BuildPanel::finished);
    connect(m_core, &CoreConnection::devicesListed, this, &BuildPanel::setPorts);
    connect(m_core, &CoreConnection::stateChanged, this, &BuildPanel::updateFlashButton);
    connect(m_core, &CoreConnection::boardStarted, this, [this](const QString &op, const QString &port) {
        if (op != u"flash")
            return;
        m_progress->setRange(0, 0); // busy until the first byte count
        m_progress->setVisible(true);
        m_status->setText(tr("Flashing %1 to %2…").arg(m_builtProject, port));
        updateFlashButton();
    });
    connect(m_core, &CoreConnection::boardProgress, this, [this](const QString &stage, qint64 written, qint64 total) {
        if (!m_progress->isVisible())
            return;
        if (stage == u"writing" && total > 0) {
            m_progress->setRange(0, 1000);
            m_progress->setValue(int(written * 1000 / total));
            m_progress->setFormat(tr("Writing %p%"));
        } else if (stage == u"verifying") {
            m_progress->setFormat(tr("Verifying"));
        } else if (stage == u"connecting") {
            m_progress->setFormat(tr("Connecting to the bootloader"));
        }
    });
    connect(m_core, &CoreConnection::boardFinished, this,
            [this](const QString &op, bool ok, const QString &error, const QJsonObject &result) {
                if (op != u"flash")
                    return;
                m_progress->setVisible(false);
                m_status->setText(ok ? tr("Flashed %1 bytes. The board is running the new firmware.")
                                           .arg(result.value(u"written"_s).toInteger())
                                     : tr("Flashing failed: %1").arg(error));
                updateFlashButton();
            });
    updateFlashButton();
}

QString BuildPanel::statusText() const
{
    return m_status->text();
}

QString BuildPanel::logText() const
{
    return m_log->toPlainText();
}

void BuildPanel::started(const QJsonObject &plan)
{
    m_log->clear();
    m_pendingProject = plan.value(u"project"_s).toString();
    m_pendingBoard = plan.value(u"board"_s).toString();
    m_status->setText(tr("Building %1 for %2…")
                          .arg(plan.value(u"project"_s).toString(), plan.value(u"board"_s).toString()));
    m_cancel->setEnabled(true);
}

void BuildPanel::appendLine(const QString &, const QString &line)
{
    m_log->appendPlainText(line);
}

void BuildPanel::finished(bool ok, const QString &error, const QString &image)
{
    m_cancel->setEnabled(false);
    m_status->setText(ok ? tr("Build succeeded. Image: %1").arg(image)
                         : tr("Build failed: %1").arg(error));
    if (ok) {
        m_builtProject = m_pendingProject;
        m_builtBoard = m_pendingBoard;
    }
    updateFlashButton();
}

void BuildPanel::setPorts(const QJsonArray &ports)
{
    const QString current = m_port->currentData().toString();
    m_port->clear();
    for (const QJsonValue &v : ports) {
        const QString path = v.toObject().value(u"path"_s).toString();
        m_port->addItem(path, path);
    }
    const int i = m_port->findData(current);
    if (i >= 0)
        m_port->setCurrentIndex(i);
    updateFlashButton();
}

void BuildPanel::updateFlashButton()
{
    m_flash->setEnabled(!m_builtProject.isEmpty() && m_port->count() > 0
                        && m_core->state() == CoreConnection::State::Connected && !m_core->isBoardBusy());
}
