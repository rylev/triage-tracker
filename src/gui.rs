use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
use tui::backend::TermionBackend;
use tui::style::{Color, Modifier, Style};
use tui::text::Span;
use tui::widgets::{BarChart, Block, Borders};
use tui::Terminal;

#[allow(dead_code)]
pub(crate) async fn gui(issues: Vec<(chrono::NaiveDate, crate::Issues)>) -> crate::Result<()> {
    let data = std::sync::Arc::new(
        issues
            .into_iter()
            .map(|(date, issues)| {
                (
                    date.format("%Y-%m-%d").to_string(),
                    issues.opened().count() as u64,
                )
            })
            .collect::<Vec<_>>(),
    );

    // Terminal initialization
    let stdout = std::io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    enum Event {
        Key(termion::event::Key),
        Tick,
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let stdin = std::io::stdin();
    use termion::input::TermRead;
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        for evt in stdin.keys() {
            if let Ok(key) = evt {
                if let Err(_) = tx_clone.send(Event::Key(key)).await {
                    return;
                }
            }
        }
    });
    tokio::spawn(async move {
        loop {
            if let Err(_) = tx.send(Event::Tick).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });
    loop {
        match rx.recv().await {
            Some(Event::Key(termion::event::Key::Char('q'))) | None => {
                break;
            }
            _ => {}
        }
        let data = data.clone();
        terminal.draw(move |f| {
            let size = f.size();
            let d = data
                .iter()
                .map(|(s, n)| (s.as_str(), *n))
                .collect::<Vec<(&str, u64)>>();
            let chart = BarChart::default()
                .block(
                    Block::default()
                        .title(Span::styled(
                            "Issues Opened",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .borders(Borders::ALL),
                )
                .bar_width(10)
                .bar_style(Style::default().fg(Color::LightBlue))
                .data(d.as_slice());
            f.render_widget(chart, size);
        })?;
    }
    rx.close();
    Ok(())
}
