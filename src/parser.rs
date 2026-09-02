use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guess {
    Movie {
        title: String,
        year: Option<u32>,
    },
    Episode {
        series: String,
        season: u32,
        episode: u32,
        episode_end: Option<u32>,
        year: Option<u32>,
    },
}

// S01E02, S1E2, s01.e02
static SEASON_EPISODE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bS(\d{1,2})[\s._-]?E(\d{1,3})(?:[\s._-]?E(\d{1,3}))?\b").unwrap()
});
// Four or more episode digits are ambiguous (for example, E0304).
// Check before parsing so a valid prefix cannot hide an invalid second episode.
static AMBIGUOUS_EPISODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bS\d{1,2}[\s._-]?E(?:\d+[\s._-]?E)*\d{4,}\b").unwrap());
// 1x02, 12x345
static SEASON_EPISODE_X: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(\d{1,2})x(\d{1,3})\b").unwrap());
// "Season 1 Episode 2"
static SEASON_EPISODE_WORDS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bseason[\s._-]*(\d{1,2})[\s._-]+episode[\s._-]*(\d{1,3})\b").unwrap()
});
// standalone 4-digit year, 1900-2099, usually in parens/brackets or bounded by separators
static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(19\d{2}|20\d{2})\b").unwrap());

// tags to strip once we know where the "junk" starts (resolution, codec, source, etc)
static JUNK_TAGS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            480p|720p|1080p|2160p|4k|uhd|hdr10?|dv|
            web[-._]?dl|webrip|web|bluray|blu-ray|bdrip|brrip|dvdrip|dvdscr|hdtv|hdrip|hdcam|cam|
            x264|x265|h264|h265|hevc|avc|xvid|divx|
            aac|ac3|dts|flac|mp3|atmos|
            [257]\.1|
            proper|repack|extended|remastered|unrated|directors[-._]?cut|
            yify|yts|rarbg|ettv|eztv
        )\b
    ",
    )
    .unwrap()
});

/// Replace common filename separators (dots, underscores, extra dashes) with spaces
/// so parsing/regexes work on "words" rather than raw scene-style tokens.
fn normalize_separators(stem: &str) -> String {
    let mut s = stem.replace('_', " ").replace('.', " ");
    // collapse runs of dashes used as separators (but keep single hyphenated words alone;
    // we don't try to be perfect here, this is a best-effort heuristic like mnamer's)
    s = s.replace(" - ", " ");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_junk(s: &str) -> String {
    let cut = JUNK_TAGS.split(s).next().unwrap_or(s);
    cut.trim_matches(|c: char| matches!(c, '-' | ' ' | '.' | '(' | ')' | '[' | ']' | '{' | '}'))
        .to_string()
}

fn clean_title(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_matches(|c: char| matches!(c, '-' | '.' | ' ' | '(' | ')' | '[' | ']' | '{' | '}'))
        .to_string();
    // Title Case each word, but don't mangle words that are already
    // mixed-case (e.g. "McDonald") -- simple heuristic: capitalize first
    // letter of lowercase/uppercase-all words only.
    cleaned
        .split(' ')
        .map(|w| {
            let has_upper = w.chars().any(|c| c.is_uppercase());
            let has_lower = w.chars().any(|c| c.is_lowercase());
            let is_mixed_case = has_upper && has_lower;
            if is_mixed_case && w.len() > 1 {
                // Looks intentionally stylized (McDonald, iPhone) -- leave as-is
                w.to_string()
            } else {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                    None => w.to_string(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a filename stem (no extension, no directory) into a best-effort guess
/// about whether it's a movie or a TV episode, extracting title/year/season/episode.
pub fn parse_filename(stem: &str) -> anyhow::Result<Guess> {
    let normalized = normalize_separators(stem);
    if let Some(marker) = AMBIGUOUS_EPISODE.find(&normalized) {
        anyhow::bail!(
            "ambiguous episode numbering '{}': use an explicit range such as S01E03-E04 if you mean episodes 3 through 4; file left unchanged",
            marker.as_str()
        );
    }
    Ok(parse_guess(stem))
}

fn parse_guess(stem: &str) -> Guess {
    let normalized = normalize_separators(stem);

    // Try episode patterns first, in order of specificity
    if let Some(caps) = SEASON_EPISODE.captures(&normalized) {
        return build_episode(
            &normalized,
            &caps[0],
            caps[1].parse().unwrap_or(1),
            caps[2].parse().unwrap_or(1),
            caps.get(3).and_then(|value| value.as_str().parse().ok()),
        );
    }
    if let Some(caps) = SEASON_EPISODE_WORDS.captures(&normalized) {
        return build_episode(
            &normalized,
            &caps[0],
            caps[1].parse().unwrap_or(1),
            caps[2].parse().unwrap_or(1),
            None,
        );
    }
    if let Some(caps) = SEASON_EPISODE_X.captures(&normalized) {
        return build_episode(
            &normalized,
            &caps[0],
            caps[1].parse().unwrap_or(1),
            caps[2].parse().unwrap_or(1),
            None,
        );
    }

    // Otherwise treat as a movie
    let year = YEAR
        .captures(&normalized)
        .and_then(|c| c[1].parse::<u32>().ok());
    let title_part = if let Some(m) = YEAR.find(&normalized) {
        &normalized[..m.start()]
    } else {
        &normalized[..]
    };
    let title = clean_title(&strip_junk(title_part));

    Guess::Movie {
        title: if title.is_empty() {
            normalized.trim().to_string()
        } else {
            title
        },
        year,
    }
}

fn build_episode(
    normalized: &str,
    marker: &str,
    season: u32,
    episode: u32,
    episode_end: Option<u32>,
) -> Guess {
    let idx = normalized.find(marker).unwrap_or(normalized.len());
    let series_part = &normalized[..idx];
    let year = YEAR
        .captures(series_part)
        .and_then(|c| c[1].parse::<u32>().ok());
    let series_part = if let Some(m) = YEAR.find(series_part) {
        &series_part[..m.start()]
    } else {
        series_part
    };
    let series = clean_title(&strip_junk(series_part));
    Guess::Episode {
        series: if series.is_empty() {
            normalized.trim().to_string()
        } else {
            series
        },
        season,
        episode,
        episode_end: episode_end.filter(|end| *end > episode),
        year,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_movie_with_year() {
        let g = parse_filename("The.Matrix.1999.1080p.BluRay.x264-GROUP").unwrap();
        assert_eq!(
            g,
            Guess::Movie {
                title: "The Matrix".to_string(),
                year: Some(1999)
            }
        );
    }

    #[test]
    fn parses_standard_episode() {
        let g = parse_filename("Breaking.Bad.S05E14.Ozymandias.720p.WEB-DL").unwrap();
        assert_eq!(
            g,
            Guess::Episode {
                series: "Breaking Bad".to_string(),
                season: 5,
                episode: 14,
                episode_end: None,
                year: None
            }
        );
    }

    #[test]
    fn parses_x_style_episode() {
        let g = parse_filename("The Office 3x05 Business School").unwrap();
        assert_eq!(
            g,
            Guess::Episode {
                series: "The Office".to_string(),
                season: 3,
                episode: 5,
                episode_end: None,
                year: None
            }
        );
    }

    #[test]
    fn parses_worded_episode() {
        let g = parse_filename("Fargo Season 2 Episode 1").unwrap();
        assert_eq!(
            g,
            Guess::Episode {
                series: "Fargo".to_string(),
                season: 2,
                episode: 1,
                episode_end: None,
                year: None
            }
        );
    }

    #[test]
    fn movie_without_junk() {
        let g = parse_filename("Parasite (2019)").unwrap();
        assert_eq!(
            g,
            Guess::Movie {
                title: "Parasite".to_string(),
                year: Some(2019)
            }
        );
    }

    #[test]
    fn parses_multi_episode_range() {
        let guess = parse_filename("Show.Name.S01E03-E04.1080p").unwrap();
        assert_eq!(
            guess,
            Guess::Episode {
                series: "Show Name".to_string(),
                season: 1,
                episode: 3,
                episode_end: Some(4),
                year: None,
            }
        );
    }

    #[test]
    fn rejects_ambiguous_packed_episode_numbers() {
        for name in [
            "Outlander.s01e0304",
            "Show.S01E03E0405",
            "Show.s01.e0304",
            "Show.S01E12345",
        ] {
            let error = parse_filename(name).unwrap_err().to_string();
            assert!(error.contains("ambiguous episode numbering"));
            assert!(error.contains("S01E03-E04"));
        }
    }

    #[test]
    fn preserves_valid_three_digit_episodes_and_explicit_ranges() {
        for name in ["Show.S01E123", "Show.S01E03E04", "Show.S01E03-E04"] {
            assert!(matches!(
                parse_filename(name).unwrap(),
                Guess::Episode { .. }
            ));
        }
    }
}
