use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::cell::Cell;
use std::time::{Duration, SystemTime};

const BASE_URL: &str = "https://api.themoviedb.org/3";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ATTEMPTS: usize = 3;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);

// TMDb query URLs contain the API key. Strip them before errors are wrapped
// in anyhow and printed to either the terminal or the optional audit log.
fn redact_request_url(error: reqwest::Error) -> reqwest::Error {
    error.without_url()
}

pub struct TmdbClient {
    api_key: String,
    http: reqwest::blocking::Client,
    lookups_deferred: Cell<bool>,
}

/// Metadata boundary: workflow tests can supply deterministic responses without
/// sending requests or depending on a real API key.
pub(crate) trait MetadataProvider {
    fn search_movie(&self, title: &str, year: Option<u32>) -> Result<Vec<MovieMatch>>;
    fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>>;
    fn episode_title(&self, series_id: u64, season: u32, episode: u32) -> Result<Option<String>>;
}

impl MetadataProvider for TmdbClient {
    fn search_movie(&self, title: &str, year: Option<u32>) -> Result<Vec<MovieMatch>> {
        TmdbClient::search_movie(self, title, year)
    }
    fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>> {
        TmdbClient::search_series(self, name)
    }
    fn episode_title(&self, series_id: u64, season: u32, episode: u32) -> Result<Option<String>> {
        TmdbClient::episode_title(self, series_id, season, episode)
    }
}

#[derive(Debug, Clone)]
pub struct MovieMatch {
    pub title: String,
    pub year: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SeriesMatch {
    pub id: u64,
    pub name: String,
    pub first_air_year: Option<u32>,
}

#[derive(Deserialize)]
struct SearchResponse<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct RawMovie {
    title: String,
    #[serde(default)]
    release_date: String,
}

#[derive(Deserialize)]
struct RawSeries {
    id: u64,
    name: String,
    #[serde(default)]
    first_air_date: String,
}

#[derive(Deserialize)]
struct RawEpisode {
    name: String,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("failed to create TMDb HTTP client")?;
        Ok(Self {
            api_key,
            http,
            lookups_deferred: Cell::new(false),
        })
    }

    fn year_from_date(date: &str) -> Option<u32> {
        date.get(0..4).and_then(|s| s.parse().ok())
    }

    pub fn search_movie(&self, title: &str, year: Option<u32>) -> Result<Vec<MovieMatch>> {
        let mut req = self
            .http
            .get(format!("{BASE_URL}/search/movie"))
            .query(&[("api_key", self.api_key.as_str()), ("query", title)]);
        if let Some(y) = year {
            req = req.query(&[("year", y.to_string())]);
        }
        let resp = self.send_with_retry(req)?;
        Self::check_status(&resp)?;
        let parsed: SearchResponse<RawMovie> = resp
            .json()
            .map_err(redact_request_url)
            .context("failed to parse TMDb movie response")?;
        Ok(parsed
            .results
            .into_iter()
            .map(|m| MovieMatch {
                title: m.title,
                year: Self::year_from_date(&m.release_date),
            })
            .collect())
    }

    pub fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>> {
        let req = self
            .http
            .get(format!("{BASE_URL}/search/tv"))
            .query(&[("api_key", self.api_key.as_str()), ("query", name)]);
        let resp = self.send_with_retry(req)?;
        Self::check_status(&resp)?;
        let parsed: SearchResponse<RawSeries> = resp
            .json()
            .map_err(redact_request_url)
            .context("failed to parse TMDb tv response")?;
        Ok(parsed
            .results
            .into_iter()
            .map(|s| SeriesMatch {
                id: s.id,
                name: s.name,
                first_air_year: Self::year_from_date(&s.first_air_date),
            })
            .collect())
    }

    /// Returns the episode title, if TMDb has one on file.
    pub fn episode_title(
        &self,
        series_id: u64,
        season: u32,
        episode: u32,
    ) -> Result<Option<String>> {
        let url = format!("{BASE_URL}/tv/{series_id}/season/{season}/episode/{episode}");
        let req = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())]);
        let resp = self.send_with_retry(req)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check_status(&resp)?;
        let parsed: RawEpisode = resp
            .json()
            .map_err(redact_request_url)
            .context("failed to parse TMDb episode response")?;
        Ok(Some(parsed.name))
    }

    fn check_status(resp: &reqwest::blocking::Response) -> Result<()> {
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            bail!("TMDb rejected the API key (401 Unauthorized). Check --api-key / $TMDB_API_KEY.");
        }
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            bail!("TMDb rate limit reached. Try again later.");
        }
        if !resp.status().is_success() {
            bail!("TMDb request returned HTTP {}", resp.status());
        }
        Ok(())
    }

    fn send_with_retry(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response> {
        if self.lookups_deferred.get() {
            bail!("TMDb lookups are deferred after rate limiting or a long server wait. Run again later.");
        }
        for attempt in 1..=MAX_ATTEMPTS {
            let attempt_request = request
                .try_clone()
                .context("failed to prepare TMDb request for retry")?;

            let fallback_delay = Duration::from_millis(250 * attempt as u64);
            let delay = match attempt_request.send() {
                Ok(response) => {
                    let status = response.status();
                    if !Self::is_retryable_status(status) {
                        return Ok(response);
                    }
                    let delay = retry_after_delay(
                        response
                            .headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|value| value.to_str().ok()),
                        SystemTime::now(),
                        fallback_delay,
                    );
                    if delay > MAX_RETRY_WAIT {
                        self.lookups_deferred.set(true);
                        bail!(
                            "TMDb requested a wait longer than 30 seconds; remaining lookups are deferred. Run again later."
                        );
                    }
                    if attempt == MAX_ATTEMPTS {
                        // Do not immediately send another file's request after
                        // exhausting retries on a rate-limited service.
                        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                            self.lookups_deferred.set(true);
                        }
                        return Ok(response);
                    }
                    eprintln!("TMDb temporarily unavailable; retrying in {:.2} seconds ({}/{MAX_ATTEMPTS})",
                        delay.as_secs_f64(), attempt + 1);
                    delay
                }
                Err(error) => {
                    let temporary = error.is_connect() || error.is_timeout();
                    if !temporary || attempt == MAX_ATTEMPTS {
                        return Err(redact_request_url(error)).context(format!(
                            "TMDb request failed after {attempt} attempt{}",
                            if attempt == 1 { "" } else { "s" }
                        ));
                    }
                    fallback_delay
                }
            };

            std::thread::sleep(delay);
        }

        unreachable!("retry loop always returns on its final attempt")
    }

    fn is_retryable_status(status: reqwest::StatusCode) -> bool {
        status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
    }
}

fn retry_after_delay(header: Option<&str>, now: SystemTime, fallback: Duration) -> Duration {
    let Some(value) = header.map(str::trim) else {
        return fallback;
    };
    if let Ok(seconds) = value.parse::<u64>() {
        return Duration::from_secs(seconds);
    }
    httpdate::parse_http_date(value)
        .map(|deadline| deadline.duration_since(now).unwrap_or(Duration::ZERO))
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_supports_seconds_dates_and_invalid_headers() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let fallback = Duration::from_millis(250);
        assert_eq!(
            retry_after_delay(Some("5"), now, fallback),
            Duration::from_secs(5)
        );
        let future = httpdate::fmt_http_date(now + Duration::from_secs(20));
        assert_eq!(
            retry_after_delay(Some(&future), now, fallback),
            Duration::from_secs(20)
        );
        let past = httpdate::fmt_http_date(now - Duration::from_secs(20));
        assert_eq!(
            retry_after_delay(Some(&past), now, fallback),
            Duration::ZERO
        );
        assert_eq!(retry_after_delay(Some("invalid"), now, fallback), fallback);
        assert_eq!(retry_after_delay(None, now, fallback), fallback);
        assert!(retry_after_delay(Some("120"), now, fallback) > MAX_RETRY_WAIT);
    }

    #[test]
    fn deferred_client_does_not_send_more_requests() {
        let client = TmdbClient {
            api_key: String::new(),
            http: reqwest::blocking::Client::builder()
                .no_proxy()
                .build()
                .unwrap(),
            lookups_deferred: Cell::new(true),
        };
        let request = client.http.get("https://example.invalid/");
        let error = client.send_with_retry(request).unwrap_err();
        assert!(error.to_string().contains("lookups are deferred"));
    }

    #[test]
    fn request_errors_do_not_expose_api_key_urls() {
        // Invalid headers produce a local error without making a network call.
        let error = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get("https://example.invalid/")
            .header("invalid\nheader", "value")
            .build()
            .unwrap_err()
            .with_url(
                "https://example.invalid/search?api_key=TEST_SECRET_MARKER"
                    .parse()
                    .unwrap(),
            );
        assert!(error.url().is_some());

        let error = redact_request_url(error);
        assert!(error.url().is_none());
        let wrapped = anyhow::Error::new(error).context("TMDb request failed");
        let formatted = format!("{wrapped:#}");
        assert!(formatted.contains("TMDb request failed"));
        assert!(!formatted.contains("TEST_SECRET_MARKER"));
        assert!(!format!("{wrapped:?}").contains("TEST_SECRET_MARKER"));
    }

    #[test]
    fn retries_only_temporary_http_failures() {
        assert!(TmdbClient::is_retryable_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(TmdbClient::is_retryable_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!TmdbClient::is_retryable_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(!TmdbClient::is_retryable_status(
            reqwest::StatusCode::NOT_FOUND
        ));
    }
}
