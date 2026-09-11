use crate::tmdb::{SeasonEpisode, SeriesMatch};
use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;
use std::time::Duration;

const BASE_URL: &str = "https://api.tvmaze.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ATTEMPTS: usize = 3;

pub(crate) struct TvmazeClient {
    http: Client,
}

#[derive(Deserialize)]
struct SearchResult {
    show: RawShow,
}

#[derive(Deserialize)]
struct RawShow {
    id: u64,
    name: String,
    premiered: Option<String>,
}

#[derive(Deserialize)]
struct RawEpisode {
    season: u32,
    number: Option<u32>,
    name: String,
    airdate: Option<String>,
}

impl TvmazeClient {
    pub(crate) fn new() -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("mnamer-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to create TVmaze HTTP client")?;
        Ok(Self { http })
    }

    pub(crate) fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>> {
        let request = self
            .http
            .get(format!("{BASE_URL}/search/shows"))
            .query(&[("q", name)]);
        let response = self.send_with_retry(request)?;
        Self::check_status(&response)?;
        let parsed: Vec<SearchResult> = response
            .json()
            .context("failed to parse TVmaze series search response")?;
        Ok(parsed
            .into_iter()
            .map(|result| SeriesMatch {
                id: result.show.id,
                name: result.show.name,
                first_air_year: result
                    .show
                    .premiered
                    .as_deref()
                    .and_then(Self::year_from_date),
            })
            .collect())
    }

    pub(crate) fn series_by_id(&self, series_id: u64) -> Result<Option<SeriesMatch>> {
        let request = self.http.get(format!("{BASE_URL}/shows/{series_id}"));
        let response = self.send_with_retry(request)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check_status(&response)?;
        let show: RawShow = response
            .json()
            .context("failed to parse TVmaze series response")?;
        Ok(Some(SeriesMatch {
            id: show.id,
            name: show.name,
            first_air_year: show.premiered.as_deref().and_then(Self::year_from_date),
        }))
    }

    pub(crate) fn season_episodes(
        &self,
        series_id: u64,
        season: u32,
    ) -> Result<Vec<SeasonEpisode>> {
        let request = self
            .http
            .get(format!("{BASE_URL}/shows/{series_id}/episodes"));
        let response = self.send_with_retry(request)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        Self::check_status(&response)?;
        let parsed: Vec<RawEpisode> = response
            .json()
            .context("failed to parse TVmaze episode response")?;
        Ok(parsed
            .into_iter()
            .filter(|episode| episode.season == season)
            .filter_map(|episode| {
                Some(SeasonEpisode {
                    episode_number: episode.number?,
                    name: episode.name,
                    air_date: episode.airdate.unwrap_or_default(),
                })
            })
            .collect())
    }

    fn year_from_date(date: &str) -> Option<u32> {
        date.get(0..4).and_then(|year| year.parse().ok())
    }

    fn check_status(response: &Response) -> Result<()> {
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            bail!("TVmaze rate limit reached. Try again later.");
        }
        if !response.status().is_success() {
            bail!("TVmaze request returned HTTP {}", response.status());
        }
        Ok(())
    }

    fn send_with_retry(&self, request: RequestBuilder) -> Result<Response> {
        for attempt in 1..=MAX_ATTEMPTS {
            let attempt_request = request
                .try_clone()
                .context("failed to prepare TVmaze request for retry")?;
            match attempt_request.send() {
                Ok(response) if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    if attempt == MAX_ATTEMPTS {
                        return Ok(response);
                    }
                    std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                }
                Ok(response) if response.status().is_server_error() => {
                    if attempt == MAX_ATTEMPTS {
                        return Ok(response);
                    }
                    std::thread::sleep(Duration::from_millis(250 * attempt as u64));
                }
                Ok(response) => return Ok(response),
                Err(error) if attempt == MAX_ATTEMPTS => {
                    return Err(error).context("TVmaze request failed")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(250 * attempt as u64)),
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_and_episode_payloads() {
        let search: Vec<SearchResult> = serde_json::from_str(
            r#"[{"score":1.0,"show":{"id":42,"name":"Example","premiered":"2026-01-02"}}]"#,
        )
        .unwrap();
        assert_eq!(search[0].show.id, 42);
        assert_eq!(TvmazeClient::year_from_date("2026-01-02"), Some(2026));

        let episode: RawEpisode = serde_json::from_str(
            r#"{"season":2,"number":3,"name":"A Title","airdate":"2026-09-11"}"#,
        )
        .unwrap();
        assert_eq!(episode.number, Some(3));
        assert_eq!(episode.name, "A Title");
    }
}
