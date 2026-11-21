#include "ProjectView.h"

#include "CoreConnection.h"
#include "Labels.h"

#include <QAction>
#include <QComboBox>
#include <QStandardPaths>
#include <QLocale>
#include <QGuiApplication>
#include <QFileDialog>
#include <QDateTime>
#include <QClipboard>
#include <QDesktopServices>
#include <QToolButton>
#include <QMenu>
#include <QDir>
#include <QFontDatabase>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QIcon>
#include <QKeySequence>
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
    auto *first = new QWidget(m_stack);
    m_placeholder = new QLabel(first);
    m_placeholder->setAlignment(Qt::AlignCenter);
    m_placeholder->setWordWrap(true);
    m_placeholder->setEnabled(false);
    // In the guide: see something work at once, without a board.
    m_try = new QPushButton(QIcon::fromTheme(u"media-playback-start"_s), tr("Try Blink in the simulator"), first);
    m_try->setToolTip(tr("Pick the Blink project, build it (after asking) and run it in the simulator"));
    m_try->hide();
    connect(m_try, &QPushButton::clicked, this, &ProjectView::tryRequested);
    auto *firstLayout = new QVBoxLayout(first);
    firstLayout->addStretch(1);
    firstLayout->addWidget(m_placeholder);
    firstLayout->addWidget(m_try, 0, Qt::AlignHCenter);
    firstLayout->addStretch(1);
    m_stack->addWidget(first);

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
    // For the user's own project: its source files (local.get's sources), each opened
    // in the editor the system has for it. The app has no editor of its own.
    m_edit = new QToolButton(page);
    m_edit->setText(tr("Edit"));
    m_edit->setIcon(QIcon::fromTheme(u"document-edit"_s));
    m_edit->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    m_edit->setPopupMode(QToolButton::InstantPopup);
    m_edit->setMenu(new QMenu(m_edit));
    m_edit->setToolTip(tr("Open one of the project's source files in your editor"));
    m_edit->hide();
    connect(m_openSite, &QPushButton::clicked, this, [this] {
        if (m_local) {
            QDesktopServices::openUrl(QUrl::fromLocalFile(m_project.value(u"path"_s).toString()));
            return;
        }
        const QUrl url(m_project.value(u"url"_s).toString());
        if (url.scheme() == u"https")
            QDesktopServices::openUrl(url);
    });

    m_build = new QPushButton(QIcon::fromTheme(u"run-build"_s), tr("Build"), page);
    m_buildAction = new QAction(m_build->icon(), tr("Build"), this);
    m_buildAction->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_B));
    setBuildToolTip(tr("Build this project for the selected board with arduino-cli"));
    connect(m_build, &QPushButton::clicked, m_buildAction, &QAction::trigger);
    connect(m_buildAction, &QAction::triggered, this, [this] {
        // The core asks for consent first; MainWindow shows that dialog.
        m_core->startBuild(m_project.value(u"id"_s).toString(), m_board->currentData().toString());
    });
    connect(m_core, &CoreConnection::stateChanged, this, &ProjectView::updateBuildButton);
    connect(m_core, &CoreConnection::buildStarted, this, &ProjectView::updateBuildButton);
    connect(m_core, &CoreConnection::buildFinished, this, &ProjectView::updateBuildButton);
    connect(m_core, &CoreConnection::buildConsentRequired, this, &ProjectView::updateBuildButton);

    m_board = new QComboBox(page);
    m_board->setAccessibleName(tr("Board"));
    connect(m_board, &QComboBox::currentIndexChanged, this, &ProjectView::showBuild);
    connect(m_board, &QComboBox::currentIndexChanged, this, &ProjectView::updateBuildButton);
    connect(m_board, &QComboBox::activated, this, [this] { m_boardChosen = true; });
    connect(m_core, &CoreConnection::devicesListed, this, [this](const QJsonArray &ports) {
        QStringList chips;
        for (const QJsonValue &v : ports) {
            const QJsonArray c = v.toObject().value(u"chips"_s).toArray();
            if (c.size() == 1 && !chips.contains(c.at(0).toString()))
                chips << c.at(0).toString();
        }
        setConnectedChips(chips);
    });

    m_tabs = new QTabWidget(page);

    auto *partsPage = new QWidget(m_tabs);
    m_parts = new QTreeWidget(partsPage);
    m_parts->setAccessibleName(tr("Parts"));
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
    m_wiring->setAccessibleName(tr("Wiring"));
    m_wiring->setHeaderLabels({tr("From"), tr("To"), tr("Note")});
    m_wiring->setRootIsDecorated(false);
    m_wiring->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    m_pins = new QLabel(wiringPage);
    m_pins->setWordWrap(true);
    auto *wiringLayout = new QVBoxLayout(wiringPage);
    wiringLayout->addWidget(m_wiring, 1);
    wiringLayout->addWidget(m_pins);
    m_tabs->addTab(wiringPage, tr("Wiring"));

    auto *codePage = new QWidget(m_tabs);
    m_code = readOnlyText(codePage, true);
    m_code->setAccessibleName(tr("Code"));
    m_copyCode = new QPushButton(QIcon::fromTheme(u"edit-copy"_s), tr("Copy code"), codePage);
    m_copyCode->setToolTip(tr("Copy the sketch, to paste it into the Arduino IDE"));
    connect(m_copyCode, &QPushButton::clicked, this, [this] { QGuiApplication::clipboard()->setText(m_code->toPlainText()); });
    m_saveCode = new QPushButton(QIcon::fromTheme(u"document-save"_s), tr("Save code…"), codePage);
    m_saveCode->setToolTip(tr("Save the sketch as a folder the Arduino IDE opens"));
    connect(m_saveCode, &QPushButton::clicked, this, [this] {
        const QString dir = QFileDialog::getExistingDirectory(this, tr("Save the sketch in which folder?"),
                                                              QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation));
        if (!dir.isEmpty())
            m_core->exportSketch(m_project.value(u"id"_s).toString(), m_board->currentData().toString(), dir);
    });
    auto *codeButtons = new QHBoxLayout;
    codeButtons->addStretch(1);
    codeButtons->addWidget(m_copyCode);
    codeButtons->addWidget(m_saveCode);
    auto *codeLayout = new QVBoxLayout(codePage);
    codeLayout->setContentsMargins(0, 0, 0, 0);
    codeLayout->addWidget(m_code, 1);
    codeLayout->addLayout(codeButtons);
    m_tabs->addTab(codePage, tr("Code"));
    m_output = readOnlyText(m_tabs, true);
    m_output->setAccessibleName(tr("Expected output"));
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
    head->addWidget(m_build, 0, Qt::AlignTop);
    head->addWidget(m_edit, 0, Qt::AlignTop);
    head->addWidget(m_openSite, 0, Qt::AlignTop);
    auto *boardRow = new QFormLayout;
    m_boardLabel = new QLabel(tr("Board:"), page);
    boardRow->addRow(m_boardLabel, m_board);
    m_buildState = new QLabel(page);
    m_buildState->setWordWrap(true);
    boardRow->addRow(QString(), m_buildState);
    // Whether the project shown is built for the board shown (build.status), so Run
    // knows whether it builds first.
    connect(m_core, &CoreConnection::buildStatusReady, this,
            [this](const QString &project, const QString &board, const QJsonObject &status) {
                if (project == m_project.value(u"id"_s).toString() && board == m_board->currentData().toString())
                    showBuildState(status);
            });
    connect(m_core, &CoreConnection::buildFinished, this, &ProjectView::requestBuildState);
    connect(m_board, &QComboBox::currentIndexChanged, this, &ProjectView::requestBuildState);
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
    connect(m_core, &CoreConnection::localLoaded, this, &ProjectView::showLocal);
    connect(m_core, &CoreConnection::requestFailed, this,
            [this](const QString &method, const QString &message) {
                if ((method == u"library.get" || method == u"local.get") && !m_requestedId.isEmpty())
                    showPlaceholder(tr("Could not load the project: %1").arg(message));
            });
    showGuide();
}

void ProjectView::setBuildToolTip(const QString &text)
{
    m_build->setToolTip(text);
    m_buildAction->setToolTip(text);
}

void ProjectView::showGuide()
{
    m_placeholder->setTextFormat(Qt::RichText);
    m_placeholder->setAlignment(Qt::AlignLeft | Qt::AlignVCenter);
    m_placeholder->setContentsMargins(24, 0, 24, 0);
    m_placeholder->setText(
        tr("<h3>Getting started</h3>"
           "<ol>"
           "<li>Pick a project from the list, or open one of your own ESP-IDF project folders.</li>"
           "<li>Choose your board, then press <b>Build</b>.</li>"
           "<li>Press <b>Run in simulator</b> to try it without hardware, or plug in a board, "
           "choose its port and press <b>Flash</b>.</li>"
           "</ol>"
           "<p>The first build needs the build tools: <b>Tools → Toolchain setup</b> installs them.</p>"));
    m_placeholder->setEnabled(true);
    m_try->setVisible(m_tryAvailable);
    m_stack->setCurrentIndex(0);
    updateBuildButton();
}

QString ProjectView::currentProjectTitle() const
{
    return m_stack->currentIndex() == 1 ? m_title->text() : QString();
}

void ProjectView::setTryAvailable(bool available)
{
    m_tryAvailable = available;
    if (m_stack->currentIndex() == 0 && m_placeholder->isEnabled())
        m_try->setVisible(available);
}

QString ProjectView::currentProjectId() const
{
    return m_stack->currentIndex() == 1 ? m_project.value(u"id"_s).toString() : QString();
}

QString ProjectView::currentBuildBoard() const
{
    return m_stack->currentIndex() == 1 ? m_board->currentData().toString() : QString();
}

QString ProjectView::currentBoardName() const
{
    return m_stack->currentIndex() == 1 ? m_board->currentText() : QString();
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
    m_placeholder->setTextFormat(Qt::PlainText);
    m_try->hide();
    m_placeholder->setAlignment(Qt::AlignCenter);
    m_placeholder->setContentsMargins(0, 0, 0, 0);
    m_placeholder->setText(text);
    m_placeholder->setEnabled(false);
    m_stack->setCurrentIndex(0);
    updateBuildButton();
}

void ProjectView::showProject(const QString &id)
{
    if (id.isEmpty())
        return;
    m_requestedId = id;
    if (m_project.value(u"id"_s).toString() != id)
        showPlaceholder(tr("Loading…"));
    // The core gives local projects ids starting "local-" (local.open).
    if (id.startsWith(u"local-"))
        m_core->getLocal(id);
    else
        m_core->getProject(id);
}

QString ProjectView::summaryText() const
{
    return m_summary->text();
}

void ProjectView::showLocal(const QJsonObject &result)
{
    const QJsonObject p = result.value(u"project"_s).toObject();
    if (!m_requestedId.isEmpty() && p.value(u"id"_s).toString() != m_requestedId)
        return;
    const QString keepBoard = m_wantBoard.isEmpty() && m_project.value(u"id"_s) == p.value(u"id"_s)
        ? m_board->currentData().toString()
        : m_wantBoard;
    m_wantBoard.clear();
    m_local = true;
    m_project = p;
    m_builds = {};
    const QJsonArray boards = result.value(u"boards"_s).toArray();
    for (const QJsonValue &v : boards)
        m_builds.append(QJsonObject{{u"board"_s, v.toObject().value(u"id"_s)}, {u"pins"_s, QJsonArray{}}});

    const QString path = p.value(u"path"_s).toString();
    const QJsonObject idf = result.value(u"idf"_s).toObject();
    const bool exists = result.value(u"exists"_s).toBool();
    m_localBuildable = exists && !idf.isEmpty();
    m_title->setText(p.value(u"name"_s).toString());
    m_meta->setText(tr("ESP-IDF project · %1").arg(path));
    if (!exists)
        m_summary->setText(tr("The project is no longer in %1. Forget it from the library's list, or put it back.").arg(path));
    else if (idf.isEmpty())
#if defined(Q_OS_WIN)
        m_summary->setText(tr("ESP-IDF was not found, so this project cannot be built yet. Install it with "
                              "Espressif's installer, or as Espressif's guide does into %USERPROFILE%\\esp\\esp-idf, "
                              "or set IDF_PATH, then select the project again."));
#elif defined(Q_OS_MACOS)
        m_summary->setText(tr("ESP-IDF was not found, so this project cannot be built yet. Install it as Espressif's "
                              "guide does, into ~/esp/esp-idf, or set IDF_PATH, then select the project again."));
#else
        m_summary->setText(tr("ESP-IDF was not found, so this project cannot be built yet. Install it (the esp-idf "
                              "package, or Espressif's guide into ~/esp/esp-idf) or set IDF_PATH, then select the "
                              "project again."));
#endif
    else
        m_summary->setText(tr("Builds with ESP-IDF %1 (%2) for the chip you choose. Build files go to the app's "
                              "cache; the project folder is not changed. Then flash it, or run it in the simulator, "
                              "from the Build dock.")
                               .arg(idf.value(u"version"_s).toString(), idf.value(u"path"_s).toString()));

    m_openSite->setText(tr("Open folder"));
    m_openSite->setIcon(QIcon::fromTheme(u"folder-open"_s));
    m_edit->menu()->clear();
    for (const QJsonValue &v : result.value(u"sources"_s).toArray()) {
        const QString rel = v.toString();
        m_edit->menu()->addAction(rel, this, [path, rel] {
            QDesktopServices::openUrl(QUrl::fromLocalFile(QDir(path).filePath(rel)));
        });
    }
    m_edit->setVisible(exists && !m_edit->menu()->isEmpty());
    setBuildToolTip(tr("Build this project for the selected chip with ESP-IDF"));
    m_tabs->hide();

    const QSignalBlocker block(m_board);
    m_board->clear();
    for (const QJsonValue &v : boards) {
        const QJsonObject b = v.toObject();
        // An ESP-IDF build is for a chip, and runs on any board with it.
        m_board->addItem(Labels::chipName(b.value(u"chip"_s).toString()), b.value(u"id"_s).toString());
    }
    m_boardLabel->setText(tr("Chip:"));
    m_board->setEnabled(m_builds.size() > 1);
    const int start = qMax(0, m_board->findData(keepBoard));
    m_board->setCurrentIndex(start);
    showBuild(start);
    m_stack->setCurrentIndex(1);
    updateBuildButton();
}

void ProjectView::showResult(const QJsonObject &result)
{
    const QJsonObject p = result.value(u"project"_s).toObject();
    // Replies can arrive after the user has moved on; show only the latest pick.
    if (!m_requestedId.isEmpty() && p.value(u"id"_s).toString() != m_requestedId)
        return;
    // Showing the same project again keeps its board, unless a link asked for one.
    const QString keepBoard = m_wantBoard.isEmpty() && m_project.value(u"id"_s) == p.value(u"id"_s)
        ? m_board->currentData().toString()
        : m_wantBoard;
    m_wantBoard.clear();
    m_local = false;
    m_project = p;
    m_builds = p.value(u"builds"_s).toArray();
    m_openSite->setText(tr("Open on esp32bench.dev"));
    m_openSite->setIcon(QIcon::fromTheme(u"internet-web-browser"_s));
    m_edit->hide();
    setBuildToolTip(tr("Build this project for the selected board with arduino-cli"));
    m_tabs->show();
    m_boardLabel->setText(tr("Board:"));

    m_title->setText(p.value(u"name"_s).toString());
    const QString level = p.value(u"level"_s).toString();
    const QString levelText = level == u"Starter" ? tr("Starter")
                              : level == u"Intermediate" ? tr("Intermediate")
                              : level == u"Advanced" ? tr("Advanced")
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
        m_board->setItemData(m_board->count() - 1, chipOf(result, id), ChipRole);
    }
    m_board->setEnabled(m_builds.size() > 1);
    m_boardChosen = !keepBoard.isEmpty();
    int start = qMax(0, m_board->findData(keepBoard));
    // A new project with a board connected that says its chip: start on a build for it.
    if (keepBoard.isEmpty())
        start = qMax(0, buildForConnectedChip());
    m_board->setCurrentIndex(start);
    showBuild(start);
    m_stack->setCurrentIndex(1);
    updateBuildButton();
}

QString ProjectView::chipOf(const QJsonObject &result, const QString &board)
{
    for (const QJsonValue &v : result.value(u"boards"_s).toArray()) {
        if (v.toObject().value(u"id"_s).toString() == board)
            return v.toObject().value(u"chip"_s).toString();
    }
    return {};
}

int ProjectView::buildForConnectedChip() const
{
    for (const QString &chip : m_connectedChips) {
        const int i = m_board->findData(chip, ChipRole);
        if (i >= 0)
            return i;
    }
    return -1;
}

void ProjectView::setConnectedChips(const QStringList &chips)
{
    m_connectedChips = chips;
    // A board plugged in while a library project is shown: switch to a build for its
    // chip, unless the board was chosen (by hand or by a link) or already fits.
    if (m_local || m_boardChosen || m_stack->currentIndex() != 1 || chips.isEmpty())
        return;
    if (chips.contains(m_board->currentData(ChipRole).toString()))
        return;
    const int i = buildForConnectedChip();
    if (i >= 0)
        m_board->setCurrentIndex(i);
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
    m_copyCode->setVisible(code.isString());
    m_saveCode->setVisible(code.isString());
    m_code->setPlainText(code.isString()
                             ? code.toString()
                             : tr("This project ships as prebuilt firmware, so there is no sketch to show."));
    QStringList sample;
    for (const QJsonValue &v : b.value(u"sample"_s).toArray())
        sample << v.toString();
    m_output->setPlainText(sample.join(u'\n'));
    emit boardSelected(m_project.value(u"id"_s).toString(), b.value(u"board"_s).toString(),
                       b.value(u"pins"_s).toArray());
}

void ProjectView::requestBuildState()
{
    m_buildState->clear();
    const QString id = m_project.value(u"id"_s).toString();
    const QString board = m_board->currentData().toString();
    if (!id.isEmpty() && !board.isEmpty() && m_core->state() == CoreConnection::State::Connected)
        m_core->requestBuildStatus(id, board);
}

void ProjectView::showBuildState(const QJsonObject &status)
{
    const QString id = m_project.value(u"id"_s).toString();
    const QString board = m_board->currentData().toString();
    const bool prebuilt = !m_local && m_project.value(u"firmware"_s).toString() != u"sketch";
    if (!status.value(u"built"_s).toBool()) {
        m_buildState->setText(prebuilt ? tr("Not downloaded for this board yet.") : tr("Not built for this board yet."));
        emit buildCurrent(id, board, false);
        return;
    }
    const QString when = QLocale().toString(QDateTime::fromSecsSinceEpoch(status.value(u"builtAt"_s).toInteger()),
                                            QLocale::ShortFormat);
    if (m_local) {
        // Its files may have changed since, so it is built again before running.
        m_buildState->setText(tr("Last built %1.").arg(when));
        emit buildCurrent(id, board, false);
    } else if (status.value(u"current"_s).toBool()) {
        m_buildState->setText(prebuilt ? tr("Downloaded %1; ready to run or flash.").arg(when)
                                       : tr("Built %1; ready to run or flash.").arg(when));
        emit buildCurrent(id, board, true);
    } else {
        m_buildState->setText(tr("Built %1, but the library has changed since; it will be built again.").arg(when));
        emit buildCurrent(id, board, false);
    }
}

void ProjectView::updateBuildButton()
{
    const bool sketch = m_project.value(u"firmware"_s).toString() == u"sketch";
    // Prebuilt firmware the library links for this board is downloaded instead of built
    // (build.start does either, after asking).
    const int b = m_board->currentIndex();
    const bool download = !m_local && !sketch && b >= 0 && b < m_builds.size()
                          && m_builds.at(b).toObject().contains(u"image"_s);
    const bool buildable = m_local ? m_localBuildable : (sketch || download);
    const QString text = download ? tr("Download") : tr("Build");
    m_build->setText(text);
    m_buildAction->setText(text);
    if (download)
        setBuildToolTip(tr("Download this board's ready-made firmware from esp32bench.dev and check it, "
                           "to run it in the simulator or flash it"));
    m_build->setVisible(m_local || sketch || download);
    m_build->setEnabled(buildable && m_core->state() == CoreConnection::State::Connected
                        && !m_core->isBuilding());
    m_buildAction->setEnabled(m_stack->currentIndex() == 1 && m_build->isVisibleTo(this) && m_build->isEnabled());
    emit currentChanged();
}

QString ProjectView::buildStateText() const
{
    return m_buildState->text();
}
