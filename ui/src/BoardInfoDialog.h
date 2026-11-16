// What board.inspect found: chip, eFuse security state and partition table.

#pragma once

#include <QDialog>
#include <QJsonObject>

class QLabel;
class QTreeWidget;

class BoardInfoDialog : public QDialog {
    Q_OBJECT

public:
    explicit BoardInfoDialog(QWidget *parent = nullptr);

    // Render a board.finished result from board.inspect.
    void showResult(const QString &port, const QJsonObject &result);
    QString summaryText() const;
    QTreeWidget *partitionTree() const { return m_partitions; }

private:
    QLabel *m_summary;
    QTreeWidget *m_partitions;
};
