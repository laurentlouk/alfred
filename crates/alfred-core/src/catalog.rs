//! What this Mac offers beyond the built-in tools: the user's Shortcuts, and whether Alfred can operate
//! apps on screen and web pages. The router and the model's schema both read it.

/// The optional capabilities found at start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// The user's shortcut names, as the Shortcuts app shows them.
    pub shortcuts: Vec<String>,
    /// `agent-desktop` is available, so `use_app` can be offered.
    pub hands: bool,
    /// `agent-browser` is available, so `use_browser` can be offered.
    pub browser: bool,
}

impl Catalog {
    /// The shortcut a spoken name stands for: the same name ignoring case, accents and punctuation, or else
    /// the only shortcut whose name contains it. `None` when nothing matches or several do.
    pub fn find_shortcut(&self, spoken: &str) -> Option<&str> {
        let want = fold(spoken);
        if want.is_empty() {
            return None;
        }
        if let Some(exact) = self.shortcuts.iter().find(|s| fold(s) == want) {
            return Some(exact);
        }
        if want.chars().count() < 4 {
            return None;
        }
        let mut hits = self.shortcuts.iter().filter(|s| fold(s).contains(&want));
        match (hits.next(), hits.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    }

    /// The shortcut whose name is exactly `spoken` (ignoring case, accents and punctuation).
    pub fn exact_shortcut(&self, spoken: &str) -> Option<&str> {
        let want = fold(spoken);
        self.shortcuts.iter().find(|s| !want.is_empty() && fold(s) == want).map(String::as_str)
    }
}

/// Lowercase, accents removed, punctuation dropped, spaces collapsed: "Chauffage  salon (confort)!" →
/// "chauffage salon confort".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let mapped: &str = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
            'ç' => "c",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => "o",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'ý' | 'ÿ' => "y",
            'œ' => "oe",
            'æ' => "ae",
            c if c.is_alphanumeric() => {
                out.push(c);
                continue;
            }
            _ => " ",
        };
        out.push_str(mapped);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog {
            shortcuts: ["Focus Timer", "Chauffage salon éco", "Chauffage salon confort", "démarre la musique", "Bonjour", "Réveil"]
                .map(String::from)
                .to_vec(),
            ..Catalog::default()
        }
    }

    #[test]
    fn folding_ignores_case_accents_and_punctuation() {
        assert_eq!(fold("Démarre  l'écran !"), "demarre l ecran");
        assert_eq!(fold("Œuvre ÇA"), "oeuvre ca");
    }

    #[test]
    fn shortcuts_match_exactly_or_by_a_unique_part() {
        let c = catalog();
        assert_eq!(c.find_shortcut("demarre la musique"), Some("démarre la musique"));
        assert_eq!(c.find_shortcut("musique"), Some("démarre la musique"));
        assert_eq!(c.find_shortcut("salon confort"), Some("Chauffage salon confort"));
        assert_eq!(c.find_shortcut("chauffage salon"), None, "two shortcuts contain it");
        assert_eq!(c.find_shortcut("foc"), None, "too short to match a part");
        assert_eq!(c.find_shortcut("reveil"), Some("Réveil"));
        assert_eq!(c.find_shortcut("vacuum"), None);
        assert_eq!(c.exact_shortcut("focus timer"), Some("Focus Timer"));
        assert_eq!(c.exact_shortcut("timer"), None);
    }
}
