use std::collections::HashMap;

use log::debug;
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

mod error;
mod github;
mod gui;

use error::Error;

#[derive(StructOpt, Debug)]
struct App {
    #[structopt(subcommand)]
    command: Command,
}

#[derive(StructOpt, Debug)]
enum Command {
    /// Track net closings of issues
    Closings(ClosingsCommand),
    /// Track triaged issues
    Triaged(TriagedCommand),
}

#[derive(StructOpt, Debug)]
enum ClosingsCommand {
    /// Print open and closed issues for a specific date
    Date { date: String },
    /// Print open and closed issues for a range of dates
    Range {
        #[structopt(short, long)]
        start: String,
        #[structopt(short, long)]
        end: String,
    },
}

#[derive(StructOpt, Debug)]
struct TriagedCommand {
    tags: Vec<String>,
    #[structopt(short, long)]
    since: Option<String>,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let app = App::from_args();
    let result = match app.command {
        Command::Closings(ClosingsCommand::Date { date }) => {
            let date = date.parse::<chrono::NaiveDate>().unwrap();
            handle_date(date).await
        }
        Command::Closings(ClosingsCommand::Range { start, end }) => {
            let start = start.parse::<chrono::NaiveDate>().unwrap();
            let end = end.parse::<chrono::NaiveDate>().unwrap();
            handle_range(start, end).await
        }
        Command::Triaged(TriagedCommand { tags, since }) => {
            let since = since.map(|s| s.parse::<chrono::NaiveDate>().unwrap());
            handle_triaged(tags, since).await
        }
    };
    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }
}

struct TriageCache {
    internal: HashMap<u32, TriageCacheLine>,
}
#[derive(Debug, Serialize, Deserialize)]
struct TriageCacheLine {
    /// The type of activity.
    activity: Activity,
    last_checked: chrono::DateTime<chrono::Utc>,
}

// Type of activity we know about an issue
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
enum Activity {
    /// We have not seen an activity since the given date, but
    /// there might be some before
    NoActivitySince(chrono::NaiveDate),
    /// The last comment was on the given date
    LastCommented(chrono::NaiveDate),
}

/// The result of looking up in the cache
enum CacheResult {
    Fresh(Activity),
    Stale(Activity),
    NotFound,
}

impl TriageCache {
    async fn from_disk() -> Result<Self> {
        let internal = match tokio::fs::read_to_string("./database/triage.json").await {
            Ok(f) => serde_json::from_str::<HashMap<u32, TriageCacheLine>>(&f).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let None = internal {
            if let Err(e) = tokio::fs::write("./database/triage.json", "{}").await {
                debug!("Writing empty cache failed: {}", e);
            }
        }

        Ok(Self {
            internal: internal.unwrap_or_default(),
        })
    }

    /// Get the cached activity for an issue
    fn get(&self, issue_number: &u32, ttl: Option<chrono::Duration>) -> CacheResult {
        self.internal
            .get(issue_number)
            .map(|l| {
                debug!("Issue #{} found in triage cache", issue_number);
                let now = chrono::Utc::now();
                let ago = ttl.map(|ttl| now - ttl);
                if ago.map(|ago| l.last_checked < ago).unwrap_or(false) {
                    CacheResult::Stale(l.activity)
                } else {
                    CacheResult::Fresh(l.activity)
                }
            })
            .unwrap_or(CacheResult::NotFound)
    }

    fn insert(&mut self, issue_number: u32, activity: Activity) {
        debug!("Inserting issue #{} into cache", issue_number);
        let cache_line = TriageCacheLine {
            activity,
            last_checked: chrono::Utc::now(),
        };
        self.internal.insert(issue_number, cache_line);
    }

    async fn flush(&self) -> Result<()> {
        let cache = serde_json::to_vec(&self.internal).unwrap();
        if let Err(e) = tokio::fs::write("./database/triage.json", cache).await {
            debug!("Writting cache failed: {}", e);
        }
        Ok(())
    }
}

async fn handle_triaged(tags: Vec<String>, since: Option<chrono::NaiveDate>) -> Result<()> {
    let mut untriaged = Vec::new();
    let mut cache = TriageCache::from_disk().await?;
    let since = since.unwrap_or_else(|| {
        let today = chrono::Local::today().naive_local();
        today - chrono::Duration::days(365)
    });
    let result = match perform_triage_loop(&tags, since, &mut untriaged, &mut cache).await {
        r @ Ok(()) | r @ Err(Error::RateLimited) => {
            cache.flush().await?;
            r
        }
        Err(e) => return Err(e),
    };
    if let Err(Error::RateLimited) = result {
        eprintln!("Error: hit Github rate limiting. Stop early");
    }
    println!(
        "{} untriaged issue{} found:",
        untriaged.len(),
        if untriaged.len() != 1 { "s" } else { "" }
    );
    for issue in untriaged {
        println!("https://github.com/rust-lang/rust/issues/{}", issue.number);
    }
    Ok(())
}

/// Check which issues given with `tags` were last active before the `last_active_yard_stick`
async fn perform_triage_loop(
    tags: &[String],
    last_active_yard_stick: chrono::NaiveDate,
    untriaged: &mut Vec<Issue>,
    cache: &mut TriageCache,
) -> Result<()> {
    for page in 1.. {
        let issues = github::fetch_issue_page(
            page,
            100,
            &tags,
            github::SortedBy::Comments,
            github::Direction::OldestFirst,
        )
        .await?;
        if issues.is_empty() {
            debug!("No more issues in page. Breaking...");
            break;
        }
        for issue in issues {
            if issue.comments == 0 {
                // Issue has no comments
                debug!("Issue #{} has no comments", issue.number);
                let created_at = issue.created_at.date().naive_local();
                let issue_number = issue.number;
                if created_at < last_active_yard_stick {
                    debug!(
                        "Issue #{} without comments was created before selected date",
                        issue_number
                    );
                    untriaged.push(issue);
                }
                continue;
            }

            match cache.get(&issue.number, Some(chrono::Duration::days(1))) {
                CacheResult::Fresh(Activity::LastCommented(last_comment)) => {
                    let issue_number = issue.number;
                    let direction = if last_comment < last_active_yard_stick {
                        untriaged.push(issue);
                        "before"
                    } else {
                        "after"
                    };
                    debug!(
                        "Issue #{} was last commented on ({:?}) {} the yard stick ({:?})",
                        issue_number, last_comment, direction, last_active_yard_stick
                    );
                    // We have an answer so go on to next issue
                    continue;
                }
                CacheResult::Fresh(Activity::NoActivitySince(no_activity_since)) => {
                    if no_activity_since <= last_active_yard_stick {
                        debug!(
                            "Issue #{} was last active (sometime before {:?}) before the yard stick ({:?})",
                            issue.number, no_activity_since, last_active_yard_stick
                        );

                        untriaged.push(issue);

                        // We have an answer so go on to next issue
                        continue;
                    } else {
                        debug!(
                            "The yard stick ({:?}) is before when we have visibility ({:?}) on issue #{}",
                            last_active_yard_stick, no_activity_since, issue.number
                        );
                        // We don't know when the issue was last active, we need to determine that
                    }
                }
                CacheResult::Stale(Activity::LastCommented(last_commented))
                    if last_commented > last_active_yard_stick =>
                {
                    // Even though the result is stale, we still know that there is a comment more recent than
                    // the yard stick. It's possible there's an even *more* recent comment, but that's not relevant.
                    continue;
                }
                _ => {
                    debug!("Issue #{} not found in cache.", issue.number);
                }
            }

            debug!(
                "State of issue #{} could not be determined from cache. Fetching comments...",
                issue.number
            );

            let comments =
                github::fetch_comment_page(issue.number, 1, 100, Some(last_active_yard_stick))
                    .await?;
            if comments.is_empty() {
                cache.insert(
                    issue.number,
                    Activity::NoActivitySince(last_active_yard_stick),
                );
                untriaged.push(issue);
            } else if comments.len() < 100 {
                cache.insert(
                    issue.number,
                    Activity::LastCommented(
                        comments.last().unwrap().created_at.naive_local().date(),
                    ),
                );
            } else {
                todo!("More than a 100 comments made in past year");
            }
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
    // TUI
    // gui::gui(issues).await?;
    let mut total: isize = 0;
    println!("Daily changes:");
    for (d, i) in issues {
        let diff = i.diff();
        total += diff;
        println!("{}: {}", d.format(" %Y-%m-%d"), diff);
    }
    println!("Total Change: {}", total);
    Ok(())
}

type Result<T> = std::result::Result<T, Error>;

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
            let answer = (days_away as u32) * Self::ESTIMATED_PAGES_PER_DAY;
            debug!(
                "Selected event page {} based on number of days away ({}) * estimated pages per day ({})",
                answer,
                days_away,
                Self::ESTIMATED_PAGES_PER_DAY
            );
            answer
        } else if days_away == 1 {
            use chrono::Timelike;
            // TODO: this will change during the day and needs to be adjusted
            debug!(
                "Selected event page 4 since date is yesterday: {} hours away",
                chrono::Utc::now().time().hour()
            );
            4
        } else {
            debug!("Selected event page 0 since date is today");
            0
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Issue {
    number: u32,
    title: String,
    comments: u32,
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
struct Comment {
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
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
    debug!("Trying to read from '{}' cache @ '{}'", cache_type, path);
    let result = tokio::fs::read(&path).await;
    if let Err(std::io::ErrorKind::NotFound) = result.as_ref().map_err(|e| e.kind()) {
        debug!("'{}' not in cache", path);
        return Ok(None);
    }
    let result = result?;

    let es = match serde_json::from_slice(&result) {
        Ok(es) => Some(es),
        Err(_) => {
            debug!("Failed to parse cache for '{}' as JSON. Deleteing...", date);
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
    debug!("Writing to cache: '{}'", path);
    let events = serde_json::to_vec(&events)?;
    Ok(tokio::fs::write(&path, &events).await?)
}

fn cache_path(date: &chrono::NaiveDate, cache_type: CacheType) -> String {
    format!("database/{}-{}.json", date.format("%Y-%m-%d"), cache_type)
}

async fn fetch_issues_for_date(date: chrono::NaiveDate) -> Result<Vec<Issue>> {
    fetch_for_date(date, |page| {
        github::fetch_issue_page(
            page,
            100,
            &[],
            github::SortedBy::Created,
            github::Direction::NewestFirst,
        )
    })
    .await
}

async fn fetch_events_for_date(date: chrono::NaiveDate) -> Result<Vec<Event>> {
    fetch_for_date(date, |page| github::fetch_event_page(page, 100)).await
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
    let mut pages_per_day = T::ESTIMATED_PAGES_PER_DAY;
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
                debug!("At least some items found in range '{:?}", r);
                let i = page.into_iter().filter(|e| e.is_relevant_for_date(&date));
                items.extend(i);
                if r.start != 0 && r.end != (page_length - 1) {
                    // The page contained all items for the date
                    debug!("All items for '{:?}' contained in page. Breaking...", date);
                    break;
                } else if r.start == 0 && fetch_index == 0 {
                    // TODO: we throw away results here that we could keep
                    debug!("In the middle of the day. Going back 1 page...");
                    page_number -= 1;
                } else if r.end != (page_length - 1) {
                    debug!("We reached the end of the date");
                    break;
                } else {
                    debug!("Date '{:?}' spans beyond page", date);
                    page_number += 1;
                    fetch_index += 1;
                }
            }
            None => {
                // No items in this page matched the date
                let most_recent = &page[0].date();
                let least_recent = &page[page_length - 1].date();
                debug!(
                    "No items for target date ({:?}) contained in page {} which spans from {:?} to {:?}",
                    date, page_number, least_recent, most_recent
                );
                if least_recent > &date {
                    debug!("The least recent item in page is further in future ({:?}) than the target date ({:?}). Going back in time...", least_recent, date);
                    let diff = (*least_recent - date).num_days() as u32;
                    let pages = diff * pages_per_day;
                    debug!(
                        "{} days in future... going back in time +{} pages",
                        diff, pages
                    );
                    page_number += pages;
                } else {
                    debug!("The most recent item in page is further in past ({:?}) than the target date ({:?}). Going forward in time...", most_recent, date);
                    let diff = (date - *most_recent).num_days() as u32;
                    let pages = diff * pages_per_day;
                    debug!(
                        "{} days in past... moving forward in time -{} pages",
                        diff, pages
                    );
                    page_number = page_number
                        .checked_sub(pages)
                        .unwrap_or_else(|| page_number - 1)
                }
                // Decrease the pages per day estimate to avoid swinging back and forth
                pages_per_day = pages_per_day.checked_add(1).unwrap_or(0);
            }
        }
    }

    Ok(items)
}
