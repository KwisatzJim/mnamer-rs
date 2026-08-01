use anyhow::{bail, Context, Result};
use serde::Deserialize;

const BASE_URL: &str = "https://api.themoviedb.org/3";

pub struct TmdbClient {
    api_key: String,
    http: reqwest::blocking::Client,
}

#[derive(Debug, Clone)]
pub struct MovieMatch {
    pub id: u64,
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
    id: u64,
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
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::blocking::Client::new(),
        }
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
        let resp = req.send().context("TMDb request failed")?;
        Self::check_status(&resp)?;
        let parsed: SearchResponse<RawMovie> = resp.json().context("failed to parse TMDb movie response")?;
        Ok(parsed
            .results
            .into_iter()
            .map(|m| MovieMatch {
                id: m.id,
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
        let resp = req.send().context("TMDb request failed")?;
        Self::check_status(&resp)?;
        let parsed: SearchResponse<RawSeries> = resp.json().context("failed to parse TMDb tv response")?;
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
    pub fn episode_title(&self, series_id: u64, season: u32, episode: u32) -> Result<Option<String>> {
        let url = format!("{BASE_URL}/tv/{series_id}/season/{season}/episode/{episode}");
        let resp = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .context("TMDb request failed")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check_status(&resp)?;
        let parsed: RawEpisode = resp.json().context("failed to parse TMDb episode response")?;
        Ok(Some(parsed.name))
    }

    fn check_status(resp: &reqwest::blocking::Response) -> Result<()> {
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            bail!("TMDb rejected the API key (401 Unauthorized). Check --api-key / $TMDB_API_KEY.");
        }
        if !resp.status().is_success() {
            bail!("TMDb request returned HTTP {}", resp.status());
        }
        Ok(())
    }
}
