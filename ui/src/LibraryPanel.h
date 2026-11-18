// The esp32bench.dev project library (library.list), with a filter and a refresh
// button, and the user's own ESP-IDF projects (local.list) first. Renders only; the
// core fetches, checks and caches the library and remembers the local projects.

#pragma once

#include <QJsonArray>
#include <QJsonObject>
#include <QWidget>

class CoreConnection;
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
    // Select a project by id (for links), now or once the list has arrived.
    void selectProject(const QString &id);
    QString statusText() const;

signals:
    // The user picked a project in the list.
    void projectSelected(const QString &id);

private:
    void applyFilter();
    void addLocalItems();
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
    bool m_autoRefreshed = false; // fetched once automatically when nothing was cached
};
