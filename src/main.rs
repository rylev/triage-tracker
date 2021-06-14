use reqwest::Client;
use serde::{Deserialize, Serialize};

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
    fn date(&self) -> chrono::Date<chrono::Utc>;
    fn is_relevant_for_date(&self, date: &chrono::Date<chrono::Utc>) -> bool;
}

trait Paged {
    // Given a date, provide the best guess as to which page to start looking at
    fn page_for_date(date: chrono::Date<chrono::Utc>) -> u32;
}

impl Dated for Event {
    fn date(&self) -> chrono::Date<chrono::Utc> {
        self.when.date()
    }

    fn is_relevant_for_date(&self, date: &chrono::Date<chrono::Utc>) -> bool {
        !matches!(self.id, EventId::Unknown) && &self.when.date() == date
    }
}

const ESTIMATED_PAGES_PER_DAY: u32 = 7;
impl Paged for Event {
    fn page_for_date(date: chrono::Date<chrono::Utc>) -> u32 {
        let days_away = (chrono::Utc::today() - date).num_days();
        if days_away > 1 {
            (days_away as u32) * ESTIMATED_PAGES_PER_DAY
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
    fn date(&self) -> chrono::Date<chrono::Utc> {
        self.created_at.date()
    }

    fn is_relevant_for_date(&self, date: &chrono::Date<chrono::Utc>) -> bool {
        &self.created_at.date() == date
    }
}

impl Paged for Issue {
    fn page_for_date(date: chrono::Date<chrono::Utc>) -> u32 {
        let days_away = (chrono::Utc::today() - date).num_days();
        if days_away > 1 {
            (days_away as u32) * ESTIMATED_PAGES_PER_DAY
        } else if days_away == 1 {
            use chrono::Timelike;
            // TODO: this will change during the day and needs to be adjusted
            println!(
                "Selected yesterday: {} hours away",
                chrono::Utc::now().time().hour()
            );
            1
        } else {
            0
        }
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

#[tokio::main]
async fn main() -> Result<()> {
    let date = chrono::Utc::today().pred();
    let items = Issues::for_date(date).await?;

    println!("Opened: ");
    for i in items.opened() {
        println!("  {}", i);
    }
    println!("Closed: ");
    for i in items.closed() {
        println!("  {}", i);
    }
    Ok(())
}

struct Issues {
    items: Vec<IssueOrEvent>,
}

impl Issues {
    async fn for_date(date: chrono::Date<chrono::Utc>) -> Result<Self> {
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
}

async fn events_for_date(date: chrono::Date<chrono::Utc>) -> Result<Vec<Event>> {
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

async fn issues_for_date(date: chrono::Date<chrono::Utc>) -> Result<Vec<Issue>> {
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
    date: &chrono::Date<chrono::Utc>,
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
    date: &chrono::Date<chrono::Utc>,
    events: &Vec<T>,
    cache_type: CacheType,
) -> Result<()> {
    let path = cache_path(date, cache_type);
    println!("Writing to cache: '{}'", path);
    let events = serde_json::to_vec(&events)?;
    Ok(tokio::fs::write(&path, &events).await?)
}

fn cache_path(date: &chrono::Date<chrono::Utc>, cache_type: CacheType) -> String {
    format!("database/{}-{}.json", date.format("%Y-%m-%d"), cache_type)
}

async fn fetch_issues_for_date(date: chrono::Date<chrono::Utc>) -> Result<Vec<Issue>> {
    fetch_for_date(date, |page| fetch_issue_page(page, 100)).await
}

async fn fetch_events_for_date(date: chrono::Date<chrono::Utc>) -> Result<Vec<Event>> {
    fetch_for_date(date, |page| fetch_event_page(page, 100)).await
}

async fn fetch_for_date<T, F, Fut>(date: chrono::Date<chrono::Utc>, fetch: F) -> Result<Vec<T>>
where
    T: Dated + Paged,
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>>>,
{
    let today = chrono::Utc::today();
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
                    todo!("Handle when the date spans page before first page fetched");
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
                println!("First event in page from '{:?}'", first);
                println!("Last event in page from '{:?}'", last);
                todo!("Handle when the page does not contain any dates");
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
