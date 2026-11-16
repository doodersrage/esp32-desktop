#include "BuildPanel.h"

#include "CoreConnection.h"

#include <QFontDatabase>
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
{
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
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(top);
    layout->addWidget(m_log, 1);

    connect(m_core, &CoreConnection::buildStarted, this, &BuildPanel::started);
    connect(m_core, &CoreConnection::buildLog, this, &BuildPanel::appendLine);
    connect(m_core, &CoreConnection::buildFinished, this, &BuildPanel::finished);
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
}
