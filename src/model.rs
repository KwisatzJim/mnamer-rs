use std::path::PathBuf;

pub(crate) struct RenamePlan {
    pub(crate) source: PathBuf,
    pub(crate) destination: PathBuf,
    pub(crate) subtitles: Vec<SubtitlePlan>,
}

pub(crate) struct SubtitlePlan {
    pub(crate) source: PathBuf,
    pub(crate) destination: PathBuf,
}
