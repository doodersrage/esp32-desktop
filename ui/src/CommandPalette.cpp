#include "CommandPalette.h"

#include <QKeyEvent>
#include <QLineEdit>
#include <QListWidget>
#include <QVBoxLayout>

#include <algorithm>

using namespace Qt::StringLiterals;

namespace {
constexpr int IndexRole = Qt::UserRole + 1;
}

CommandPalette::CommandPalette(QWidget *parent)
    : QDialog(parent, Qt::Popup | Qt::FramelessWindowHint)
    , m_field(new QLineEdit(this))
    , m_list(new QListWidget(this))
{
    setWindowTitle(tr("Find an action or project"));
    m_field->setPlaceholderText(tr("Type to find an action, a project or a panel"));
    m_field->setAccessibleName(tr("Find an action or project"));
    m_list->setAccessibleName(tr("Matches"));
    m_list->setUniformItemSizes(true);
    connect(m_field, &QLineEdit::textChanged, this, &CommandPalette::filter);
    connect(m_field, &QLineEdit::returnPressed, this, &CommandPalette::runSelected);
    connect(m_list, &QListWidget::itemActivated, this, &CommandPalette::runSelected);
    // Up and Down in the field move through the list.
    m_field->installEventFilter(this);
    auto *layout = new QVBoxLayout(this);
    layout->addWidget(m_field);
    layout->addWidget(m_list, 1);
    resize(520, 360);
}

void CommandPalette::setEntries(QList<Entry> entries)
{
    m_entries = std::move(entries);
    m_field->clear();
    filter();
}

void CommandPalette::filter()
{
    const QStringList words = m_field->text().toLower().split(u' ', Qt::SkipEmptyParts);
    m_list->clear();
    for (qsizetype i = 0; i < m_entries.size(); ++i) {
        const Entry &e = m_entries.at(i);
        const QString hay = (e.text + u' ' + e.detail).toLower();
        if (!std::all_of(words.begin(), words.end(), [&](const QString &w) { return hay.contains(w); }))
            continue;
        auto *item = new QListWidgetItem(e.detail.isEmpty() ? e.text : u"%1    —  %2"_s.arg(e.text, e.detail), m_list);
        item->setData(IndexRole, int(i));
    }
    m_list->setCurrentRow(0);
}

QStringList CommandPalette::shown() const
{
    QStringList out;
    for (int i = 0; i < m_list->count(); ++i)
        out << m_entries.at(m_list->item(i)->data(IndexRole).toInt()).text;
    return out;
}

void CommandPalette::runSelected()
{
    QListWidgetItem *item = m_list->currentItem();
    if (!item)
        return;
    const auto run = m_entries.at(item->data(IndexRole).toInt()).run;
    accept();
    if (run)
        run();
}

bool CommandPalette::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == m_field && event->type() == QEvent::KeyPress) {
        const int key = static_cast<QKeyEvent *>(event)->key();
        if (key == Qt::Key_Down || key == Qt::Key_Up) {
            const int row = std::clamp(m_list->currentRow() + (key == Qt::Key_Down ? 1 : -1), 0,
                                       std::max(0, m_list->count() - 1));
            m_list->setCurrentRow(row);
            return true;
        }
    }
    return QDialog::eventFilter(watched, event);
}
