//! Which clicks can't be taken back, and which fields Alfred never types into. The loops never perform
//! such clicks: they stop and Alfred asks first.

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
    "shut down",
    "restart",
    "lock screen",
    "force quit",
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
    "éteindre",
    "eteindre",
    "redémarrer",
    "redemarrer",
    "fermer la session",
    "verrouiller",
    "forcer à quitter",
    "éteindre",
    "eteindre",
    "redémarrer",
    "redemarrer",
    "fermer la session",
    "verrouiller",
    "forcer à quitter",
];

/// What commits on a web page, beyond [`RISKY`]: orders, bookings, sign-ups, donations.
const WEB_RISKY: &[&str] = &[
    "place order",
    "checkout",
    "check out",
    "buy now",
    "pay",
    "purchase",
    "subscribe",
    "sign up",
    "register",
    "book",
    "reserve",
    "donate",
    "confirm",
    "submit",
    "send",
    "post",
    "publish",
    "delete",
    "unsubscribe",
    // French
    "passer la commande",
    "passer commande",
    "abonner",
    "abonnez",
    "inscrire",
    "inscrivez",
    "réserver",
    "reserver",
    "faire un don",
];

/// Fields for secrets: passwords, card details, bank numbers, sign-in codes, national ids.
const SECRET_FIELDS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "passcode",
    "pin",
    "card number",
    "cardnumber",
    "card no",
    "cc number",
    "credit card",
    "debit card",
    "cardholder",
    "cvc",
    "cvv",
    "cvc2",
    "cvv2",
    "csc",
    "security code",
    "card code",
    "expiry",
    "expiration date",
    "iban",
    "bic",
    "swift",
    "routing number",
    "account number",
    "one time",
    "otp",
    "verification code",
    "authentication code",
    "2fa",
    "two factor",
    "ssn",
    "social security",
    // French
    "mot de passe",
    "carte bancaire",
    "numéro de carte",
    "numero de carte",
    "cryptogramme",
    "code de sécurité",
    "code de securite",
    "date d expiration",
    "code de vérification",
    "code de verification",
    "sécurité sociale",
    "securite sociale",
];

/// `autocomplete` values a page gives its secret fields.
const SECRET_AUTOCOMPLETE: &[&str] =
    &["current-password", "new-password", "one-time-code", "cc-number", "cc-csc", "cc-exp", "cc-exp-month", "cc-exp-year", "cc-name"];

fn has_word(text: &str, words: &[&str]) -> bool {
    let padded = format!(" {} ", alfred_core::catalog::fold(text));
    words.iter().any(|w| padded.contains(&format!(" {} ", alfred_core::catalog::fold(w))))
}

/// True when clicking an element with this name might do something that can't be undone.
pub fn is_risky(name: &str) -> bool {
    has_word(name, RISKY)
}

/// [`is_risky`], or a web page's commit buttons ("Place order", "Book", "Sign up", "Donate").
pub fn is_risky_on_web(name: &str) -> bool {
    is_risky(name) || has_word(name, WEB_RISKY)
}

/// True when a field is for a secret Alfred never types: judged by its role, its name, the hint beside it
/// (`name:pwd`), and its `type` and `autocomplete` attributes when the page gives them.
pub fn is_secret_field(role: &str, name: &str, hint: &str, input_type: &str, autocomplete: &str) -> bool {
    let input_type = input_type.trim().to_lowercase();
    let autocomplete = autocomplete.trim().to_lowercase();
    input_type == "password"
        || autocomplete.split_whitespace().any(|a| SECRET_AUTOCOMPLETE.contains(&a))
        || has_word(role, &["password"])
        || has_word(name, SECRET_FIELDS)
        || has_word(hint, SECRET_FIELDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irreversible_names_are_caught_in_both_languages() {
        for n in [
            "Send",
            "Send Message",
            "Envoyer",
            "Delete",
            "Move to Trash",
            "Supprimer le message",
            "Pay now",
            "Sign Out",
            "Publier",
            "Shut Down…",
            "Éteindre…",
            "Redémarrer",
            "Fermer la session Ada",
            "Verrouiller l’écran",
        ] {
            assert!(is_risky(n), "{n}");
        }
    }

    #[test]
    fn web_commit_buttons_are_caught() {
        for n in [
            "Place order",
            "Proceed to checkout",
            "Buy now",
            "Pay €12.99",
            "Subscribe",
            "Sign up",
            "Book this room",
            "Donate",
            "Confirm booking",
            "S'abonner",
            "Réserver",
            "Passer la commande",
        ] {
            assert!(is_risky_on_web(n), "{n}");
        }
        for n in
            ["Search", "Next page", "Add to list", "Books", "Facebook", "Sign in", "Read more", "Compare prices", "Show 50 results", ""]
        {
            assert!(!is_risky_on_web(n), "{n}");
        }
    }

    #[test]
    fn secret_fields_are_known_by_name_hint_type_or_autocomplete() {
        assert!(is_secret_field("textbox", "Password", "", "", ""));
        assert!(is_secret_field("textbox", "", "name:pwd", "", ""), "a nameless field whose input is called pwd");
        assert!(is_secret_field("textbox", "", "", "password", ""));
        assert!(is_secret_field("textbox", "1234 1234 1234 1234", "", "text", "cc-number"));
        assert!(is_secret_field("textbox", "CVC", "", "", ""));
        assert!(is_secret_field("textbox", "", "name:otp", "", "one-time-code"));
        assert!(is_secret_field("textbox", "IBAN", "", "", ""));
        assert!(is_secret_field("textbox", "Mot de passe", "", "", ""));
        assert!(is_secret_field("textbox", "Social Security Number", "", "", ""));
        assert!(is_secret_field("textbox", "Enter the verification code", "", "", ""));
        for (role, name, hint) in [
            ("searchbox", "Search Wikipedia", "name:search"),
            ("textbox", "Email", ""),
            ("textbox", "Your message", "name:msg"),
            ("combobox", "Pinned tabs", ""),
        ] {
            assert!(!is_secret_field(role, name, hint, "text", ""), "{name}");
        }
    }

    #[test]
    fn everyday_controls_pass() {
        for n in ["Reply", "Répondre", "New Message", "Inbox", "Sender", "Subject", "Message body", "Search", "Sending options", ""] {
            assert!(!is_risky(n), "{n}");
        }
    }
}
