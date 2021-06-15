use clap::{App, Arg};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> Result<()> {
    let matches = App::new("Triage Tracker")
        .arg(
            Arg::with_name("date")
                .short("d")
                .long("date")
                .value_name("DATE")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("start")
                .short("s")
                .long("start")
                .value_name("DATE")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("end")
                .short("e")
                .long("end")
                .value_name("DATE")
                .takes_value(true),
        )
        .get_matches();
    match (
        matches.value_of("date"),
        matches.value_of("start"),
        matches.value_of("end"),
    ) {
        (Some(d), None, None) => {
            let date = d.parse::<chrono::NaiveDate>().unwrap();
            handle_date(date).await?;
        }
        (None, Some(s), Some(e)) => {
            let start = s.parse::<chrono::NaiveDate>().unwrap();
            let end = e.parse::<chrono::NaiveDate>().unwrap();
            handle_range(start, end).await?;
        }
        _ => {
            eprintln!("INVALID ARGS! TODO: Print help");
        }
    }
    Ok(())
}

async fn handle_date(date: chrono::NaiveDate) -> Result<()> {
    let items = Issues::for_date(date).await?;

    println!("On {}", date.format("%Y-%m-%d"));
    let opened = items.opened().collect::<Vec<_>>();
    println!("{} opened: ", opened.len());
    for i in opened {
        println!("  {}", i);
    }
    let closed = items.closed().collect::<Vec<_>>();
    println!("{} closed: ", closed.len());
    for i in items.closed() {
        println!("  {}", i);
    }
    Ok(())
}

use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;
use tui::backend::TermionBackend;
use tui::style::{Color, Modifier, Style};
use tui::text::Span;
use tui::widgets::{BarChart, Block, Borders};
use tui::Terminal;

async fn handle_range(start: chrono::NaiveDate, end: chrono::NaiveDate) -> Result<()> {
    if end >= start {
        return Err("--start must be more recent than --end".into());
    }
    let mut issues = Vec::new();
    let mut date = start;
    loop {
        issues.push((date, Issues::for_date(date).await?));
        date = date.pred();
        if date == end.pred() {
            break;
        }
    }
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

type BoxedError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxedError>;

#[derive(Debug)]
enum IssueOrEvent {
    Issue(Issue),
    Event(Event),
}

impl IssueOrEvent {
    fn issue(&self) -> &Issue {
        match self {
            Self::Issue(i) => i,
            Self::Event(e) => &e.issue,
        }
    }

    fn state_change(&self) -> StateChange {
        match self {
            Self::Issue(_) => StateChange::Opened,
            Self::Event(e) => match &e.id {
                EventId::Closed => StateChange::Closed,
                EventId::Reopened => StateChange::Opened,
                _ => panic!("Invalid event"),
            },
        }
    }
}

#[derive(Debug)]
enum StateChange {
    Opened,
    Closed,
}

#[derive(Serialize, Deserialize, Debug)]
struct Event {
    actor: Actor,
    #[serde(rename = "event")]
    id: EventId,
    issue: Issue,
    #[serde(rename = "created_at")]
    when: chrono::DateTime<chrono::Utc>,
}

impl Event {
    fn is_pull_request(&self) -> bool {
        self.issue.is_pull_request()
    }
}

trait Dated {
    fn date(&self) -> chrono::NaiveDate;
    fn is_relevant_for_date(&self, date: &chrono::NaiveDate) -> bool;
}

trait Paged {
    const ESTIMATED_PAGES_PER_DAY: u32;
    // Given a date, provide the best guess as to which page to start looking at
    fn page_for_date(date: chrono::NaiveDate) -> u32;
}

impl Dated for Event {
    fn date(&self) -> chrono::NaiveDate {
        self.when.date().naive_utc()
    }

    fn is_relevant_for_date(&self, date: &chrono::NaiveDate) -> bool {
        !matches!(self.id, EventId::Unknown) && &self.date() == date
    }
}

impl Paged for Event {
    const ESTIMATED_PAGES_PER_DAY: u32 = 4;
    fn page_for_date(date: chrono::NaiveDate) -> u32 {
        let days_away = (chrono::Utc::today().naive_utc() - date).num_days();
        if days_away > 1 {
            (days_away as u32) * Self::ESTIMATED_PAGES_PER_DAY
        } else if days_away == 1 {
            use chrono::Timelike;
            // TODO: this will change during the day and needs to be adjusted
            println!(
                "Selected yesterday: {} hours away",
                chrono::Utc::now().time().hour()
            );
            4
        } else {
            0
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Issue {
    number: u32,
    title: String,
    pull_request: Option<PullRequest>,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl Issue {
    fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

impl Dated for Issue {
    fn date(&self) -> chrono::NaiveDate {
        self.created_at.date().naive_utc()
    }

    fn is_relevant_for_date(&self, date: &chrono::NaiveDate) -> bool {
        &self.date() == date
    }
}

impl Paged for Issue {
    const ESTIMATED_PAGES_PER_DAY: u32 = 1;
    fn page_for_date(date: chrono::NaiveDate) -> u32 {
        let days_away = (chrono::Utc::today().naive_utc() - date).num_days();
        (days_away as u32) / 6
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!("#{}: {}", self.number, self.title))
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct PullRequest {}

#[derive(Serialize, Deserialize, Debug)]
enum EventId {
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "reopened")]
    Reopened,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug)]
struct Actor {
    login: String,
}

struct Issues {
    items: Vec<IssueOrEvent>,
}

impl Issues {
    async fn for_date(date: chrono::NaiveDate) -> Result<Self> {
        let (events, issues) = tokio::join!(events_for_date(date), issues_for_date(date));
        let events = events?;
        let issues = issues?;
        let mut items = Vec::with_capacity(events.len() + issues.len());
        items.extend(
            events
                .into_iter()
                .filter(|i| !i.is_pull_request())
                .map(IssueOrEvent::Event),
        );
        items.extend(
            issues
                .into_iter()
                .filter(|i| !i.is_pull_request())
                .map(IssueOrEvent::Issue),
        );
        items.sort_by(|i1, i2| i1.issue().number.cmp(&i2.issue().number));
        items.dedup_by(|i1, i2| i1.issue().number == i2.issue().number);
        Ok(Self { items })
    }

    fn opened(&self) -> impl Iterator<Item = &Issue> {
        self.items
            .iter()
            .filter(|i| matches!(i.state_change(), StateChange::Opened))
            .map(|i| i.issue())
    }

    fn closed(&self) -> impl Iterator<Item = &Issue> {
        self.items
            .iter()
            .filter(|i| matches!(i.state_change(), StateChange::Closed))
            .map(|i| i.issue())
    }

    #[allow(dead_code)]
    fn diff(&self) -> isize {
        let opened = self.opened().count() as isize;
        let closed = self.closed().count() as isize;
        opened - closed
    }
}

async fn events_for_date(date: chrono::NaiveDate) -> Result<Vec<Event>> {
    let es = match read_cache(&date, CacheType::Events).await? {
        Some(es) => es,
        None => {
            let events = fetch_events_for_date(date).await?;
            let _ = write_cache(&date, &events, CacheType::Events).await;
            events
        }
    };
    Ok(es)
}

async fn issues_for_date(date: chrono::NaiveDate) -> Result<Vec<Issue>> {
    let es = match read_cache(&date, CacheType::Issues).await? {
        Some(es) => es,
        None => {
            let issues = fetch_issues_for_date(date).await?;
            let _ = write_cache(&date, &issues, CacheType::Issues).await;
            issues
        }
    };
    Ok(es)
}

#[derive(Clone, Copy, Debug)]
enum CacheType {
    Issues,
    Events,
}

impl std::fmt::Display for CacheType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let typ = match self {
            CacheType::Issues => "issues",
            CacheType::Events => "events",
        };
        f.write_str(typ)
    }
}

async fn read_cache<T: serde::de::DeserializeOwned>(
    date: &chrono::NaiveDate,
    cache_type: CacheType,
) -> Result<Option<Vec<T>>> {
    let path = cache_path(date, cache_type);
    println!("Trying to read from '{}' cache @ '{}'", cache_type, path);
    let result = tokio::fs::read(&path).await;
    if let Err(std::io::ErrorKind::NotFound) = result.as_ref().map_err(|e| e.kind()) {
        println!("'{}' not in cache", path);
        return Ok(None);
    }
    let result = result?;

    let es = match serde_json::from_slice(&result) {
        Ok(es) => Some(es),
        Err(_) => {
            println!("Failed to parse cache for '{}' as JSON. Deleteing...", date);
            let _ = tokio::fs::remove_file(&path).await;
            None
        }
    };
    Ok(es)
}

async fn write_cache<T: Serialize>(
    date: &chrono::NaiveDate,
    events: &Vec<T>,
    cache_type: CacheType,
) -> Result<()> {
    let path = cache_path(date, cache_type);
    println!("Writing to cache: '{}'", path);
    let events = serde_json::to_vec(&events)?;
    Ok(tokio::fs::write(&path, &events).await?)
}

fn cache_path(date: &chrono::NaiveDate, cache_type: CacheType) -> String {
    format!("database/{}-{}.json", date.format("%Y-%m-%d"), cache_type)
}

async fn fetch_issues_for_date(date: chrono::NaiveDate) -> Result<Vec<Issue>> {
    fetch_for_date(date, |page| fetch_issue_page(page, 100)).await
}

async fn fetch_events_for_date(date: chrono::NaiveDate) -> Result<Vec<Event>> {
    fetch_for_date(date, |page| fetch_event_page(page, 100)).await
}

async fn fetch_for_date<T, F, Fut>(date: chrono::NaiveDate, fetch: F) -> Result<Vec<T>>
where
    T: Dated + Paged,
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>>>,
{
    let today = chrono::Utc::today().naive_utc();
    let days_away = (today - date).num_days();
    assert!(days_away >= 0);
    let mut page_number = T::page_for_date(date.clone());
    let mut items = Vec::new();
    let mut fetch_index = 0;
    loop {
        let page = fetch(page_number).await?;
        if page.is_empty() {
            break;
        }
        let page_length = page.len();
        let first = page.iter().position(|e| e.date() == date);
        let last = page.iter().rev().position(|e| e.date() == date);
        let range = first.zip(last).map(|(f, l)| f..(page_length - 1 - l));
        match range {
            Some(r) => {
                // At least some items were for this date
                println!("At least some items found in range '{:?}", r);
                let i = page.into_iter().filter(|e| e.is_relevant_for_date(&date));
                items.extend(i);
                if r.start != 0 && r.end != (page_length - 1) {
                    // The page contained all items for the date
                    println!("All items for '{:?}' contained in page. Breaking...", date);
                    break;
                }
                if r.start == 0 && fetch_index == 0 {
                    println!("In the middle of the day. Going back 1 page...");
                    page_number -= 1;
                    continue;
                }
                if r.end != (page_length - 1) {
                    println!("We reached the end of the date");
                    break;
                }
                println!("Date '{:?}' spans beyond page", date);
                page_number += 1;
                fetch_index += 1;
            }
            None => {
                // No items in this page matched the date
                println!("No items for '{:?}' contained in page", date);
                let first = &page[0].date();
                let last = &page[page_length - 1].date();
                if last > &date {
                    let diff = (*last - date).num_days() as u32;
                    let pages = diff * T::ESTIMATED_PAGES_PER_DAY;
                    println!("{} days in future... going back {} pages", diff, pages);
                    page_number += pages;
                } else {
                    println!("First item in page from '{:?}'", first);
                    println!("Last item in page from '{:?}'", last);
                    let diff = (date - *last).num_days() as u32;
                    let pages = diff * T::ESTIMATED_PAGES_PER_DAY;
                    page_number -= pages;
                    println!("{} days in future... going back {} pages", diff, pages);
                }
            }
        }
    }

    Ok(items)
}

async fn fetch_event_page(page: u32, per_page: u8) -> Result<Vec<Event>> {
    println!("Fetching event page {}", page);
    fetch_page("issues/events", page, per_page).await
}

async fn fetch_issue_page(page: u32, per_page: u8) -> Result<Vec<Issue>> {
    println!("Fetching issue page {}", page);
    fetch_page("issues", page, per_page).await
}

async fn fetch_page<T: serde::de::DeserializeOwned>(
    path: &str,
    page: u32,
    per_page: u8,
) -> Result<Vec<T>> {
    assert!(per_page <= 100);
    Ok(Client::new()
        .get(format!(
            "https://api.github.com/repos/rust-lang/rust/{}?per_page={}&page={}",
            path, per_page, page
        ))
        .header("Accept", " application/vnd.github.v3+json")
        .header("User-Agent", "rust-triage-tracker")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
