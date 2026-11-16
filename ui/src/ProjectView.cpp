#include "ProjectView.h"

#include "CoreConnection.h"
#include "Labels.h"

#include <QComboBox>
#include <QDesktopServices>
#include <QFontDatabase>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QScrollArea>
#include <QStackedWidget>
#include <QTabWidget>
#include <QTreeWidget>
#include <QUrl>
#include <QVBoxLayout>

using namespace Qt::StringLiterals;

namespace {

QPlainTextEdit *readOnlyText(QWidget *parent, bool fixedFont)
{
    auto *t = new QPlainTextEdit(parent);
    t->setReadOnly(true);
    t->setLineWrapMode(QPlainTextEdit::NoWrap);
    if (fixedFont)
        t->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    return t;
}

// A bulleted HTML list from manifest strings, escaped.
QString bullets(const QJsonArray &items)
{
    QString html;
    for (const QJsonValue &v : items)
        html += u"<li>%1</li>"_s.arg(v.toString().toHtmlEscaped());
    return html.isEmpty() ? QString() : u"<ul>%1</ul>"_s.arg(html);
}

} // namespace

ProjectView::ProjectView(CoreConnection *core, QWidget *parent)
    : QWidget(parent)
    , m_core(core)
    , m_stack(new QStackedWidget(this))
{
    // Page 0: nothing selected, or loading.
    m_placeholder = new QLabel(m_stack);
    m_placeholder->setAlignment(Qt::AlignCenter);
    m_placeholder->setWordWrap(true);
    m_placeholder->setEnabled(false);
    m_stack->addWidget(m_placeholder);

    // Page 1: the project.
    auto *page = new QWidget(m_stack);
    m_title = new QLabel(page);
    QFont big = m_title->font();
    big.setPointSizeF(big.pointSizeF() * 1.4);
    big.setBold(true);
    m_title->setFont(big);
    m_title->setWordWrap(true);
    m_meta = new QLabel(page);
    m_summary = new QLabel(page);
    m_summary->setWordWrap(true);
    m_openSite = new QPushButton(QIcon::fromTheme(u"internet-web-browser"_s), tr("Open on esp32bench.dev"), page);
    connect(m_openSite, &QPushButton::clicked, this, [this] {
        const QUrl url(m_project.value(u"url"_s).toString());
        if (url.scheme() == u"https")
            QDesktopServices::openUrl(url);
    });

    m_board = new QComboBox(page);
    connect(m_board, &QComboBox::currentIndexChanged, this, &ProjectView::showBuild);

    m_tabs = new QTabWidget(page);

    auto *partsPage = new QWidget(m_tabs);
    m_parts = new QTreeWidget(partsPage);
    m_parts->setHeaderLabels({tr("Quantity"), tr("Part")});
    m_parts->setRootIsDecorated(false);
    m_parts->header()->setSectionResizeMode(0, QHeaderView::ResizeToContents);
    m_libraries = new QLabel(partsPage);
    m_libraries->setWordWrap(true);
    auto *partsLayout = new QVBoxLayout(partsPage);
    partsLayout->addWidget(m_parts, 1);
    partsLayout->addWidget(m_libraries);
    m_tabs->addTab(partsPage, tr("Parts"));

    auto *wiringPage = new QWidget(m_tabs);
    m_wiring = new QTreeWidget(wiringPage);
    m_wiring->setHeaderLabels({tr("From"), tr("To"), tr("Note")});
    m_wiring->setRootIsDecorated(false);
    m_wiring->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_pins = new QLabel(wiringPage);
    m_pins->setWordWrap(true);
    auto *wiringLayout = new QVBoxLayout(wiringPage);
    wiringLayout->addWidget(m_wiring, 1);
    wiringLayout->addWidget(m_pins);
    m_tabs->addTab(wiringPage, tr("Wiring"));

    m_code = readOnlyText(m_tabs, true);
    m_tabs->addTab(m_code, tr("Code"));
    m_output = readOnlyText(m_tabs, true);
    m_tabs->addTab(m_output, tr("Expected output"));

    auto *notesScroll = new QScrollArea(m_tabs);
    notesScroll->setWidgetResizable(true);
    m_notes = new QLabel(notesScroll);
    m_notes->setWordWrap(true);
    m_notes->setAlignment(Qt::AlignTop | Qt::AlignLeft);
    m_notes->setTextFormat(Qt::RichText);
    m_notes->setTextInteractionFlags(Qt::TextSelectableByMouse);
    notesScroll->setWidget(m_notes);
    m_tabs->addTab(notesScroll, tr("Notes"));

    auto *head = new QHBoxLayout;
    auto *headText = new QVBoxLayout;
    headText->addWidget(m_title);
    headText->addWidget(m_meta);
    head->addLayout(headText, 1);
    head->addWidget(m_openSite, 0, Qt::AlignTop);
    auto *boardRow = new QFormLayout;
    boardRow->addRow(tr("Board:"), m_board);
    auto *pageLayout = new QVBoxLayout(page);
    pageLayout->addLayout(head);
    pageLayout->addWidget(m_summary);
    pageLayout->addLayout(boardRow);
    pageLayout->addWidget(m_tabs, 1);
    m_stack->addWidget(page);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(m_stack);

    connect(m_core, &CoreConnection::projectLoaded, this, &ProjectView::showResult);
    connect(m_core, &CoreConnection::requestFailed, this,
            [this](const QString &method, const QString &message) {
                if (method == u"library.get" && !m_requestedId.isEmpty())
                    showPlaceholder(tr("Could not load the project: %1").arg(message));
            });
    showPlaceholder(tr("Select a project to see its parts, wiring and code."));
}

QString ProjectView::title() const
{
    return m_title->text();
}

QString ProjectView::notesText() const
{
    return m_notes->text();
}

void ProjectView::showPlaceholder(const QString &text)
{
    m_placeholder->setText(text);
    m_stack->setCurrentIndex(0);
}

void ProjectView::showProject(const QString &id)
{
    if (id.isEmpty())
        return;
    m_requestedId = id;
    if (m_project.value(u"id"_s).toString() != id)
        showPlaceholder(tr("Loading…"));
    m_core->getProject(id);
}

void ProjectView::showResult(const QJsonObject &result)
{
    const QJsonObject p = result.value(u"project"_s).toObject();
    // Replies can arrive after the user has moved on; show only the latest pick.
    if (!m_requestedId.isEmpty() && p.value(u"id"_s).toString() != m_requestedId)
        return;
    m_project = p;
    m_builds = p.value(u"builds"_s).toArray();

    m_title->setText(p.value(u"name"_s).toString());
    const QString level = p.value(u"level"_s).toString();
    const QString levelText = level == u"Starter" ? tr("Starter")
                              : level == u"Intermediate" ? tr("Intermediate")
                                                         : level;
    const bool prebuilt = p.value(u"firmware"_s).toString() == u"prebuilt";
    m_meta->setText(tr("%1 · about %2 min · %3")
                        .arg(levelText)
                        .arg(p.value(u"minutes"_s).toInt())
                        .arg(prebuilt ? tr("prebuilt firmware") : tr("Arduino sketch")));
    m_summary->setText(p.value(u"summary"_s).toString());

    m_parts->clear();
    for (const QJsonValue &v : p.value(u"parts"_s).toArray()) {
        const QJsonObject part = v.toObject();
        auto *item = new QTreeWidgetItem(m_parts);
        item->setText(0, QString::number(part.value(u"quantity"_s).toInt()));
        item->setText(1, part.value(u"name"_s).toString());
    }
    QStringList libs;
    for (const QJsonValue &v : p.value(u"libraries"_s).toArray())
        libs << v.toString();
    m_libraries->setText(libs.isEmpty() ? QString() : tr("Libraries: %1").arg(libs.join(u", "_s)));

    QString notes;
    if (const QString n = bullets(p.value(u"notes"_s).toArray()); !n.isEmpty())
        notes += u"<h3>%1</h3>%2"_s.arg(tr("Notes"), n);
    if (const QString n = bullets(p.value(u"open"_s).toArray()); !n.isEmpty())
        notes += u"<h3>%1</h3>%2"_s.arg(tr("Not yet tested"), n);
    QJsonArray verifiedLines;
    for (const QJsonValue &v : p.value(u"verified"_s).toArray()) {
        const QJsonObject o = v.toObject();
        const QString when = o.value(u"when"_s).toString();
        verifiedLines.append(when.isEmpty()
                                 ? tr("%1: %2").arg(o.value(u"board"_s).toString(), o.value(u"what"_s).toString())
                                 : tr("%1: %2 (%3)").arg(o.value(u"board"_s).toString(),
                                                         o.value(u"what"_s).toString(), when));
    }
    if (const QString n = bullets(verifiedLines); !n.isEmpty())
        notes += u"<h3>%1</h3>%2"_s.arg(tr("Tested on hardware"), n);
    m_notes->setText(notes.isEmpty() ? tr("No notes for this project.").toHtmlEscaped() : notes);

    // Board names come with the result; fall back to the id.
    QHash<QString, QString> names;
    for (const QJsonValue &v : result.value(u"boards"_s).toArray()) {
        const QJsonObject b = v.toObject();
        names.insert(b.value(u"id"_s).toString(),
                     tr("%1 (%2)").arg(b.value(u"name"_s).toString(),
                                       Labels::chipName(b.value(u"chip"_s).toString())));
    }
    const QSignalBlocker block(m_board);
    m_board->clear();
    for (const QJsonValue &v : m_builds) {
        const QString id = v.toObject().value(u"board"_s).toString();
        m_board->addItem(names.value(id, id), id);
    }
    m_board->setEnabled(m_builds.size() > 1);
    showBuild(0);
    m_stack->setCurrentIndex(1);
}

void ProjectView::showBuild(int index)
{
    m_wiring->clear();
    m_code->clear();
    m_output->clear();
    m_pins->clear();
    if (index < 0 || index >= m_builds.size())
        return;
    const QJsonObject b = m_builds.at(index).toObject();
    for (const QJsonValue &v : b.value(u"wiring"_s).toArray()) {
        const QJsonObject w = v.toObject();
        auto *item = new QTreeWidgetItem(m_wiring);
        item->setText(0, w.value(u"from"_s).toString());
        item->setText(1, w.value(u"to"_s).toString());
        item->setText(2, w.value(u"note"_s).toString());
    }
    QStringList pins;
    for (const QJsonValue &v : b.value(u"pins"_s).toArray())
        pins << v.toString();
    if (!pins.isEmpty())
        m_pins->setText(tr("Pins used: %1").arg(pins.join(u", "_s)));

    const QJsonValue code = b.value(u"code"_s);
    m_code->setPlainText(code.isString()
                             ? code.toString()
                             : tr("This project ships as prebuilt firmware, so there is no sketch to show."));
    QStringList sample;
    for (const QJsonValue &v : b.value(u"sample"_s).toArray())
        sample << v.toString();
    m_output->setPlainText(sample.join(u'\n'));
}
