// Tools → Disk space: how much the app keeps on disk (cache.usage), and removing
// builds (cache.clean). The core measures and removes; this shows and asks.

#pragma once

#include <QDialog>
#include <QJsonArray>

class CoreConnection;
class QLabel;
class QPushButton;
class QTreeWidget;

class DiskSpaceDialog : public QDialog {
    Q_OBJECT

public:
    explicit DiskSpaceDialog(CoreConnection *core, QWidget *parent = nullptr);
    // Ask the core again.
    void refresh();
    QTreeWidget *tree() const { return m_tree; }
    QPushButton *removeButton() const { return m_remove; }
    QPushButton *removeAllButton() const { return m_removeAll; }
    QString statusText() const;

private:
    void showUsage(const QJsonArray &categories, const QJsonArray &builds);

    CoreConnection *m_core;
    QTreeWidget *m_tree;
    QLabel *m_status;
    QPushButton *m_remove;
    QPushButton *m_removeAll;
};
