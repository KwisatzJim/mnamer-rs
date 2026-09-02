/// Remove characters that are illegal (or just annoying) in filenames on
/// common filesystems, and trim the result.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_matches('.').trim().to_string()
}

pub struct MovieVars<'a> {
    pub title: &'a str,
    pub year: Option<u32>,
    pub ext: &'a str,
}

pub struct EpisodeVars<'a> {
    pub series: &'a str,
    pub year: Option<u32>,
    pub season: u32,
    pub episode: u32,
    pub episode_end: Option<u32>,
    pub episode_title: &'a str,
    pub ext: &'a str,
}

fn apply_case(s: String, lower: bool, scene: bool) -> String {
    let mut out = if lower { s.to_lowercase() } else { s };
    if scene {
        out = out.replace(' ', ".");
    }
    out
}

pub fn render_movie(template: &str, vars: &MovieVars, lower: bool, scene: bool) -> String {
    let year = vars
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let ext = if vars.ext.is_empty() {
        String::new()
    } else {
        format!(".{}", vars.ext)
    };
    let rendered = template
        .replace("{title}", vars.title)
        .replace("{year}", &year)
        .replace("{ext}", &ext);
    sanitize(&apply_case(rendered, lower, scene))
}

pub fn render_episode(template: &str, vars: &EpisodeVars, lower: bool, scene: bool) -> String {
    let year = vars
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let ext = if vars.ext.is_empty() {
        String::new()
    } else {
        format!(".{}", vars.ext)
    };
    let episode_end = vars
        .episode_end
        .map(|episode| format!("{episode:02}"))
        .unwrap_or_default();
    let episode_range = vars
        .episode_end
        .map(|end| format!("{:02}-E{end:02}", vars.episode))
        .unwrap_or_else(|| format!("{:02}", vars.episode));
    // Older saved templates used only {episode}. Preserve the complete
    // episode identity unless the template explicitly handles the range.
    let episode = if !template.contains("{episode_end}") && !template.contains("{episode_range}") {
        episode_range.clone()
    } else {
        format!("{:02}", vars.episode)
    };
    let rendered = template
        .replace("{series}", vars.series)
        .replace("{year}", &year)
        .replace("{season}", &format!("{:02}", vars.season))
        .replace("{episode}", &episode)
        .replace("{episode_end}", &episode_end)
        .replace("{episode_range}", &episode_range)
        .replace("{episode_title}", vars.episode_title)
        .replace("{ext}", &ext);
    sanitize(&apply_case(rendered, lower, scene))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_movie() {
        let vars = MovieVars {
            title: "The Matrix",
            year: Some(1999),
            ext: "mkv",
        };
        assert_eq!(
            render_movie("{title} ({year}){ext}", &vars, false, false),
            "The Matrix (1999).mkv"
        );
    }

    #[test]
    fn renders_episode_scene_style() {
        let vars = EpisodeVars {
            series: "Breaking Bad",
            year: None,
            season: 5,
            episode: 14,
            episode_end: None,
            episode_title: "Ozymandias",
            ext: "mkv",
        };
        assert_eq!(
            render_episode(
                "{series} - S{season}E{episode} - {episode_title}{ext}",
                &vars,
                false,
                true
            ),
            "Breaking.Bad.-.S05E14.-.Ozymandias.mkv"
        );
    }

    #[test]
    fn strips_illegal_characters() {
        assert_eq!(sanitize("Weird: Title? *Name*"), "Weird Title Name");
    }

    #[test]
    fn renders_multi_episode_range() {
        let vars = EpisodeVars {
            series: "Show Name",
            year: Some(2024),
            season: 1,
            episode: 3,
            episode_end: Some(4),
            episode_title: "Part One + Part Two",
            ext: "mkv",
        };

        assert_eq!(
            render_episode(
                "{series} - S{season}E{episode_range} - {episode_title}{ext}",
                &vars,
                false,
                false,
            ),
            "Show Name - S01E03-E04 - Part One + Part Two.mkv"
        );
        assert_eq!(
            render_episode(
                "{series} - s{season}e{episode} - {episode_title}{ext}",
                &vars,
                false,
                false,
            ),
            "Show Name - s01e03-E04 - Part One + Part Two.mkv"
        );
        assert_eq!(
            render_episode(
                "{series} - S{season}E{episode}-E{episode_end}{ext}",
                &vars,
                false,
                false,
            ),
            "Show Name - S01E03-E04.mkv"
        );
    }
}
