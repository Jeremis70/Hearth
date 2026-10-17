//! Display titles from No-Intro / Redump style file names.
//!
//! `Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!]` →
//! `The Legend of Zelda: Ocarina of Time`.

const ARTICLES: [&str; 3] = ["The", "A", "An"];

/// Inner extensions left in the stem by double extensions (`Game (USA).nkit.iso`).
const INNER_EXTENSIONS: [&str; 1] = [".nkit"];

/// Cleans a file stem (no extension) into a display title.
pub fn clean_title(stem: &str) -> String {
    let stem = INNER_EXTENSIONS
        .iter()
        .find_map(|ext| {
            let cut = stem.len().checked_sub(ext.len())?;
            let tail = stem.get(cut..)?;
            tail.eq_ignore_ascii_case(ext).then(|| &stem[..cut])
        })
        .unwrap_or(stem);
    let stripped = strip_tags(&stem.replace('_', " "));
    let mut parts: Vec<String> = stripped
        .split(" - ")
        .map(collapse_spaces)
        .filter(|p| !p.is_empty())
        .collect();
    if let Some(first) = parts.first_mut() {
        *first = move_article(first);
    }
    let title = parts.join(": ");
    if title.is_empty() {
        collapse_spaces(stem)
    } else {
        title
    }
}

/// Removes `(…)` and `[…]` groups (regions, versions, dump flags).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Legend of Zelda, The` → `The Legend of Zelda`.
fn move_article(s: &str) -> String {
    for article in ARTICLES {
        if let Some(rest) = s.strip_suffix(&format!(", {article}")) {
            return format!("{article} {rest}");
        }
    }
    s.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_intro_name_with_article_and_subtitle() {
        assert_eq!(
            clean_title("Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!]"),
            "The Legend of Zelda: Ocarina of Time"
        );
    }

    #[test]
    fn region_and_flags_are_removed() {
        assert_eq!(clean_title("Super Mario 64 (USA)"), "Super Mario 64");
        assert_eq!(
            clean_title("Mario Kart 64 (Europe) (Rev 1) [b1]"),
            "Mario Kart 64"
        );
        assert_eq!(clean_title("Banjo-Kazooie (USA) (Rev A)"), "Banjo-Kazooie");
    }

    #[test]
    fn other_articles_and_underscores() {
        assert_eq!(clean_title("Bug's Life, A (USA)"), "A Bug's Life");
        assert_eq!(clean_title("Star_Fox_64_(U)"), "Star Fox 64");
    }

    #[test]
    fn nkit_inner_extension_is_removed() {
        assert_eq!(
            clean_title("Legend of Zelda, The - Twilight Princess (USA).nkit"),
            "The Legend of Zelda: Twilight Princess"
        );
        assert_eq!(clean_title("Pikmin (USA).NKit"), "Pikmin");
    }

    #[test]
    fn plain_names_stay_as_they_are() {
        assert_eq!(clean_title("GoldenEye 007"), "GoldenEye 007");
        assert_eq!(clean_title("F-Zero X"), "F-Zero X");
    }

    #[test]
    fn name_made_only_of_tags_falls_back_to_stem() {
        assert_eq!(clean_title("(Beta)"), "(Beta)");
    }
}
