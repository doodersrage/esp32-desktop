// The esp32bench.dev project library (library.list), with a filter and a refresh
// button. Renders only; the core fetches, checks and caches the library.

#pragma once

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
    // Item data role holding the project page URL.
    static constexpr int UrlRole = Qt::UserRole + 1;

    explicit LibraryPanel(CoreConnection *core, QWidget *parent = nullptr);

    // Render a library.list result. Public so tests can feed fixed data.
    void showLibrary(const QJsonObject &library);
    QTreeWidget *tree() const { return m_tree; }
    QLineEdit *filter() const { return m_filter; }
    QString statusText() const;

private:
    void applyFilter();
    void setStatus(const QString &text);
    void updateEnabled();

    CoreConnection *m_core;
    QLineEdit *m_filter;
    QLabel *m_status;
    QToolButton *m_refresh;
    QTreeWidget *m_tree;
    bool m_autoRefreshed = false; // fetched once automatically when nothing was cached
};
