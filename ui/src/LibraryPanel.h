// The esp32bench.dev project library (library.list), with a filter and a refresh
// button, and the user's own ESP-IDF projects (local.list) first. Renders only; the
// core fetches, checks and caches the library and remembers the local projects.

#pragma once

#include <QJsonArray>
#include <QJsonObject>
#include <QWidget>

#include <utility>

class CoreConnection;
class QCheckBox;
class QComboBox;
class QLabel;
class QLineEdit;
class QToolButton;
class QTreeWidget;

class LibraryPanel : public QWidget {
    Q_OBJECT

public:
    enum Column { NameColumn, LevelColumn, TimeColumn, ChipsColumn, FirmwareColumn, ColumnCount };
    // Item data roles: the project page URL and the project id.
    static constexpr int UrlRole = Qt::UserRole + 1;
    static constexpr int IdRole = Qt::UserRole + 2;
    // True for the user's own ESP-IDF projects.
    static constexpr int LocalRole = Qt::UserRole + 3;
    // True when the project runs in the simulator (library.list's `simulator`).
    static constexpr int SimulatorRole = Qt::UserRole + 4;
    // The chip ids it has builds for.
    static constexpr int ChipsRole = Qt::UserRole + 5;
    // True when the user starred it (library.list's `starred`).
    static constexpr int StarredRole = Qt::UserRole + 6;
    // A local project's folder.
    static constexpr int PathRole = Qt::UserRole + 7;
    // Its tags, and the boards it is built for (library.list's tags and built).
    static constexpr int TagsRole = Qt::UserRole + 8;
    static constexpr int BuiltRole = Qt::UserRole + 9;

    explicit LibraryPanel(CoreConnection *core, QWidget *parent = nullptr);

    // Render a library.list result. Public so tests can feed fixed data.
    void showLibrary(const QJsonObject &library);
    // Render local.list's projects, above the library's.
    void showLocal(const QJsonArray &projects);
    // Ask for a folder and open it as a local ESP-IDF project.
    void openFolder();
    QToolButton *openButton() const { return m_open; }
    QTreeWidget *tree() const { return m_tree; }
    QLineEdit *filter() const { return m_filter; }
    QCheckBox *simulatorFilter() const { return m_simOnly; }
    QCheckBox *boardFilter() const { return m_fitsBoard; }
    QCheckBox *starredFilter() const { return m_starredOnly; }
    QCheckBox *builtFilter() const { return m_builtOnly; }
    QComboBox *tagFilter() const { return m_tag; }
    // Every project listed: its name and id (for the command palette).
    QList<std::pair<QString, QString>> projects() const;
    // Select a project by id (for links), now or once the list has arrived.
    void selectProject(const QString &id);
    QString statusText() const;
    // The list's column widths, order and hidden columns, which the main window saves
    // with its layout. Widths the user dragged are kept; until then the name column
    // fits the longest name.
    QByteArray saveColumns() const;
    void restoreColumns(const QByteArray &state);
    void resetColumns();

signals:
    // The user picked a project in the list.
    void projectSelected(const QString &id);
    // Double-click or Enter on a project: run it (or build it).
    void projectActivated(const QString &id);
    // A local project was forgotten (its folder is untouched; opening it again undoes it).
    void projectForgotten(const QString &name, const QString &path);

protected:
    void resizeEvent(QResizeEvent *event) override;

private:
    void applyFilter();
    void fillTopics();
    void addLocalItems();
    // Size the columns to their contents, unless the user has sized them.
    void fitColumns();
    void setStatus(const QString &text);
    void updateEnabled();

    CoreConnection *m_core;
    QString m_pendingSelect;
    QLineEdit *m_filter;
    QLabel *m_status;
    QToolButton *m_refresh;
    QToolButton *m_open;
    QJsonArray m_local;
    QTreeWidget *m_tree;
    QCheckBox *m_simOnly;
    QCheckBox *m_fitsBoard;
    QCheckBox *m_starredOnly;
    QCheckBox *m_builtOnly;
    QComboBox *m_tag;
    // Chips on the connected boards that say which they are (device.list).
    QStringList m_connectedChips;
    QByteArray m_defaultColumns;
    bool m_userSized = false; // the user dragged a column edge, or a saved layout did
    bool m_fitting = false;
    bool m_autoRefreshed = false; // fetched once automatically when nothing was cached
};
