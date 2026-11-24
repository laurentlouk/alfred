//! Which clicks can't be taken back. The loop never performs them: it stops and Alfred asks first.

/// Words (English and French) that name an irreversible action, matched as whole words in an element's
/// name, case- and accent-insensitively.
const RISKY: &[&str] = &[
    // English
    "send",
    "delete",
    "remove",
    "erase",
    "trash",
    "pay",
    "buy",
    "purchase",
    "order",
    "checkout",
    "submit",
    "post",
    "publish",
    "transfer",
    "unsubscribe",
    "quit",
    "sign out",
    "log out",
    "empty",
    "discard",
    "reset",
    "uninstall",
    "confirm",
    "accept",
    "approve",
    // French
    "envoyer",
    "supprimer",
    "effacer",
    "retirer",
    "corbeille",
    "payer",
    "acheter",
    "commander",
    "valider",
    "soumettre",
    "publier",
    "transférer",
    "transferer",
    "virement",
    "désabonner",
    "desabonner",
    "quitter",
    "déconnexion",
    "deconnexion",
    "vider",
    "réinitialiser",
    "reinitialiser",
    "désinstaller",
    "desinstaller",
    "confirmer",
    "accepter",
    "approuver",
];

/// True when clicking an element with this name might do something that can't be undone.
pub fn is_risky(name: &str) -> bool {
    let words = alfred_core::catalog::fold(name);
    let padded = format!(" {words} ");
    RISKY.iter().any(|w| padded.contains(&format!(" {} ", alfred_core::catalog::fold(w))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irreversible_names_are_caught_in_both_languages() {
        for n in ["Send", "Send Message", "Envoyer", "Delete", "Move to Trash", "Supprimer le message", "Pay now", "Sign Out", "Publier"] {
            assert!(is_risky(n), "{n}");
        }
    }

    #[test]
    fn everyday_controls_pass() {
        for n in ["Reply", "Répondre", "New Message", "Inbox", "Sender", "Subject", "Message body", "Search", "Sending options", ""] {
            assert!(!is_risky(n), "{n}");
        }
    }
}
