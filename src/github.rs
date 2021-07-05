use crate::*;
use log::debug;
use reqwest::Client;

pub(crate) async fn fetch_event_page(page: u32, per_page: u8) -> Result<Vec<Event>> {
    debug!("Fetching event page {}", page);
    fetch_page(
        "issues/events",
        page,
        per_page,
        &[],
        SortedBy::Created,
        Direction::NewestFirst,
    )
    .await
}

pub(crate) async fn fetch_issue_page(
    page: u32,
    per_page: u8,
    labels: &[String],
    direction: Direction,
) -> Result<Vec<Issue>> {
    debug!("Fetching issue page {}", page);
    fetch_page(
        "issues",
        page,
        per_page,
        labels,
        SortedBy::Created,
        direction,
    )
    .await
}

pub(crate) async fn fetch_comment_page(
    issue_number: u32,
    page: u32,
    per_page: u8,
    since: Option<chrono::NaiveDate>,
) -> Result<Vec<Comment>> {
    debug!("Fetching comments for issue {} page {}", issue_number, page);
    let mut params = vec![
        ("per_page", per_page.to_string()),
        ("page", page.to_string()),
    ];
    if let Some(since) = since {
        let since = chrono::NaiveDateTime::new(since, chrono::NaiveTime::from_hms(0, 0, 0));
        let since = chrono::DateTime::<chrono::Utc>::from_utc(since, chrono::Utc);
        params.push(("since", since.format("%Y-%m-%dT%H:%M:%SZ").to_string()))
    }
    fetch(&format!("issues/{}/comments", issue_number), &params).await
}

pub(crate) enum Direction {
    NewestFirst,
    OldestFirst,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NewestFirst => "desc",
            Self::OldestFirst => "asc",
        };
        f.write_str(s)
    }
}

#[allow(dead_code)]
pub(crate) enum SortedBy {
    Created,
    Updated,
    Comments,
}

impl std::fmt::Display for SortedBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Comments => "comments",
        };
        f.write_str(s)
    }
}

pub(crate) async fn fetch_page<T: serde::de::DeserializeOwned>(
    path: &str,
    page: u32,
    per_page: u8,
    labels: &[String],
    sorted_by: SortedBy,
    direction: Direction,
) -> Result<Vec<T>> {
    assert!(per_page <= 100);

    let mut params = vec![
        ("per_page", per_page.to_string()),
        ("page", page.to_string()),
        ("sort", sorted_by.to_string()),
        ("direction", direction.to_string()),
    ];
    if !labels.is_empty() {
        params.push(("labels", labels.join(",")))
    }
    // "https://api.github.com/repos/rust-lang/rust/{}?per_page={}&page={}&sort={}&direction={}{}",
    // path, per_page, page, sorted_by, direction,labels
    fetch(path, &params).await
}

pub(crate) async fn fetch<T: serde::de::DeserializeOwned>(
    path: &str,
    params: &[(&str, String)],
) -> Result<Vec<T>> {
    let params = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");
    Ok(Client::new()
        .get(format!(
            "https://api.github.com/repos/rust-lang/rust/{}?{}",
            path, params
        ))
        .header("Accept", " application/vnd.github.v3+json")
        .header("User-Agent", "rust-triage-tracker")
        .send()
        .await?
        .error_for_status()
        .map_err(|e| -> Error {
            if let Some(reqwest::StatusCode::FORBIDDEN) = e.status() {
                Error::RateLimited
            } else {
                e.into()
            }
        })?
        .json()
        .await?)
}
