use crate::cli::EpisodeApi;
use crate::tmdb::{MetadataProvider, MovieMatch, SeasonEpisode, SeriesMatch, TmdbClient};
use crate::tvmaze::TvmazeClient;
use anyhow::Result;

/// Routes movie lookups to TMDb and television lookups to the explicitly
/// selected episode provider. TMDb remains the default for compatibility.
pub(crate) struct MetadataClient {
    tmdb: TmdbClient,
    tvmaze: TvmazeClient,
    episode_api: EpisodeApi,
}

impl MetadataClient {
    pub(crate) fn new(api_key: String, episode_api: EpisodeApi) -> Result<Self> {
        Ok(Self {
            tmdb: TmdbClient::new(api_key)?,
            tvmaze: TvmazeClient::new()?,
            episode_api,
        })
    }
}

impl MetadataProvider for MetadataClient {
    fn search_movie(&self, title: &str, year: Option<u32>) -> Result<Vec<MovieMatch>> {
        self.tmdb.search_movie(title, year)
    }

    fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>> {
        match self.episode_api {
            EpisodeApi::Tmdb => self.tmdb.search_series(name),
            EpisodeApi::Tvmaze => self.tvmaze.search_series(name),
        }
    }

    fn season_episodes(&self, series_id: u64, season: u32) -> Result<Vec<SeasonEpisode>> {
        match self.episode_api {
            EpisodeApi::Tmdb => self.tmdb.season_episodes(series_id, season),
            EpisodeApi::Tvmaze => self.tvmaze.season_episodes(series_id, season),
        }
    }
}
