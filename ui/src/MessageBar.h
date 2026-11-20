// A message across the top of the main window that stays until it is closed or a new
// attempt starts: build, flash and link errors, which the status bar would show only
// for a few seconds. Theme colours and icons only.

#pragma once

#include <QFrame>

class QLabel;
class QToolButton;

class MessageBar : public QFrame {
    Q_OBJECT

public:
    explicit MessageBar(QWidget *parent = nullptr);

    void showError(const QString &text);
    // An explanation of something the board or a tool printed, with a link to the
    // site's page about it when there is one.
    void showExplanation(const QString &text, const QString &url);
    QString text() const;

private:
    QLabel *m_icon;
    QLabel *m_text;
    QToolButton *m_close;
    QString m_plain; // an explanation's text without its link
};
