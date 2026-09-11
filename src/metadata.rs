use crate::cli::EpisodeApi;
use crate::tmdb::{MetadataProvider, MovieMatch, SeasonEpisode, SeriesMatch, TmdbClient};
use crate::tvmaze::TvmazeClient;
use anyhow::{Context, Result};

/// Routes movie lookups to TMDb and television lookups to the explicitly
/// selected episode provider. TMDb remains the default for compatibility.
pub(crate) struct MetadataClient {
    tmdb: Option<TmdbClient>,
    tvmaze: TvmazeClient,
    episode_api: EpisodeApi,
}

impl MetadataClient {
    pub(crate) fn new(api_key: Option<String>, episode_api: EpisodeApi) -> Result<Self> {
        Ok(Self {
            tmdb: api_key.map(TmdbClient::new).transpose()?,
            tvmaze: TvmazeClient::new()?,
            episode_api,
        })
    }

    fn tmdb(&self) -> Result<&TmdbClient> {
        self.tmdb.as_ref().context(
            "TMDb lookup requires an API key. Pass --api-key, set $TMDB_API_KEY, or add api_key to config.toml",
        )
    }
}

impl MetadataProvider for MetadataClient {
    fn search_movie(&self, title: &str, year: Option<u32>) -> Result<Vec<MovieMatch>> {
        self.tmdb()?.search_movie(title, year)
    }

    fn search_series(&self, name: &str) -> Result<Vec<SeriesMatch>> {
        match self.episode_api {
            EpisodeApi::Tmdb => self.tmdb()?.search_series(name),
            EpisodeApi::Tvmaze => self.tvmaze.search_series(name),
        }
    }

    fn series_by_id(&self, series_id: u64) -> Result<Option<SeriesMatch>> {
        match self.episode_api {
            EpisodeApi::Tmdb => self.tmdb()?.series_by_id(series_id),
            EpisodeApi::Tvmaze => self.tvmaze.series_by_id(series_id),
        }
    }

    fn season_episodes(&self, series_id: u64, season: u32) -> Result<Vec<SeasonEpisode>> {
        match self.episode_api {
            EpisodeApi::Tmdb => self.tmdb()?.season_episodes(series_id, season),
            EpisodeApi::Tvmaze => self.tvmaze.season_episodes(series_id, season),
        }
    }
}
