use reqwest::Client;
use serde::{Deserialize, Serialize};

type BoxedError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxedError>;

#[derive(Serialize, Deserialize, Debug)]
struct Actor {
    login: String,
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

#[derive(Serialize, Deserialize, Debug)]
struct Issue {
    number: u32,
    title: String,
    pull_request: Option<PullRequest>,
}

impl Issue {
    fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
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

#[tokio::main]
async fn main() -> Result<()> {
    let date = chrono::Utc::today().pred();
    let events = events_for_date(date).await?;
    println!(
        "Pull Requests closed {:#?}",
        events
            .iter()
            .filter_map(|e| {
                if e.issue.is_pull_request() && matches!(e.id, EventId::Closed) {
                    Some(e.issue.number)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    );
    Ok(())
}

async fn events_for_date(date: chrono::Date<chrono::Utc>) -> Result<Vec<Event>> {
    let es = match read_cache(&date).await? {
        Some(es) => es,
        None => {
            let events = fetch_events_for_date(date).await?;
            let _ = write_cache(&date, &events).await;
            events
        }
    };
    Ok(es)
}

async fn read_cache(date: &chrono::Date<chrono::Utc>) -> Result<Option<Vec<Event>>> {
    let path = cache_path(date);
    println!("Trying to read from cache: '{}'", path);
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

async fn write_cache(date: &chrono::Date<chrono::Utc>, events: &Vec<Event>) -> Result<()> {
    let path = cache_path(date);
    println!("Writing to cache: '{}'", path);
    let events = serde_json::to_vec(&events)?;
    Ok(tokio::fs::write(&path, &events).await?)
}

fn cache_path(date: &chrono::Date<chrono::Utc>) -> String {
    format!("database/{}-events.json", date.format("%Y-%m-%d"))
}

async fn fetch_events_for_date(date: chrono::Date<chrono::Utc>) -> Result<Vec<Event>> {
    let today = chrono::Utc::today();
    let days_away = (today - date).num_days();
    assert!(days_away >= 0);
    let mut page_number = if days_away > 1 {
        // We assume that there are ~7 pages per day
        (days_away as u32) * 7
    } else if days_away == 1 {
        use chrono::Timelike;
        // TODO: this will change during the day and needs to be adjusted
        println!(
            "Selected yesterday: {} hours away",
            chrono::Utc::now().time().hour()
        );
        2
    } else {
        0
    };
    let mut events = Vec::new();
    let mut fetch_index = 0;
    loop {
        let page = fetch_page(page_number, 100).await?;
        if page.is_empty() {
            break;
        }
        let page_length = page.len();
        let first = page.iter().position(|e| e.when.date() == date);
        let last = page.iter().rev().position(|e| e.when.date() == date);
        let range = first.zip(last).map(|(f, l)| f..(page_length - 1 - l));
        match range {
            Some(r) => {
                // At least some events were for this date
                println!("At least some events found in range '{:?}", r);
                let i = page
                    .into_iter()
                    .filter(|e| !matches!(e.id, EventId::Unknown) && e.when.date() == date);
                events.extend(i);
                if r.start != 0 && r.end != (page_length - 1) {
                    // The page contained all events for the date
                    println!("All events for '{:?}' contained in page. Breaking...", date);
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
                // No events in this page matched the date
                println!("No events for '{:?}' contained in page", date);
                let first = &page[0].when;
                let last = &page[page_length - 1].when;
                println!("First event in page from '{:?}'", first);
                println!("Last event in page from '{:?}'", last);
                todo!("Handle when the page does not contain any dates");
            }
        }
    }

    Ok(events)
}

async fn fetch_page(page: u32, per_page: u8) -> Result<Vec<Event>> {
    println!("Fetching page {}", page);
    assert!(per_page <= 100);
    Ok(Client::new()
        .get(format!(
            "https://api.github.com/repos/rust-lang/rust/issues/events?per_page={}&page={}",
            per_page, page
        ))
        .header("Accept", " application/vnd.github.v3+json")
        .header("User-Agent", "rust-triage-tracker")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
