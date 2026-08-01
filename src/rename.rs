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
    let year = vars.year.map(|y| y.to_string()).unwrap_or_else(|| "Unknown".to_string());
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
    let year = vars.year.map(|y| y.to_string()).unwrap_or_else(|| "Unknown".to_string());
    let ext = if vars.ext.is_empty() {
        String::new()
    } else {
        format!(".{}", vars.ext)
    };
    let rendered = template
        .replace("{series}", vars.series)
        .replace("{year}", &year)
        .replace("{season}", &format!("{:02}", vars.season))
        .replace("{episode}", &format!("{:02}", vars.episode))
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
            episode_title: "Ozymandias",
            ext: "mkv",
        };
        assert_eq!(
            render_episode("{series} - S{season}E{episode} - {episode_title}{ext}", &vars, false, true),
            "Breaking.Bad.-.S05E14.-.Ozymandias.mkv"
        );
    }

    #[test]
    fn strips_illegal_characters() {
        assert_eq!(sanitize("Weird: Title? *Name*"), "Weird Title Name");
    }
}
