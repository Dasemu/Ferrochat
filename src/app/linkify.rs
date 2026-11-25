//! Finds links, e-mail addresses and @mentions in message text (pure).
//!
//! WhatsApp makes `http(s)://` URLs, `www.` addresses and bare domains
//! clickable. Only `http`, `https` and `mailto` targets are ever produced, so
//! a message cannot make the app open `file:`, `javascript:` or custom-scheme
//! URLs.

/// Longest link target handled; longer ones stay plain text.
pub const MAX_URL_LEN: usize = 2048;

/// Top-level domains accepted for scheme-less links such as `example.com/x`.
/// A short list: `file.rs` or `index.js` must not turn into links.
const TLDS: &[&str] = &[
    "com", "org", "net", "io", "dev", "app", "me", "co", "edu", "gov", "info", "biz", "ai", "tv",
    "es", "uk", "de", "fr", "it", "nl", "pt", "br", "mx", "ar", "cl", "us", "ca", "eu", "ru", "jp",
    "cn", "in", "au", "ch", "se", "no", "pl", "be", "at",
];

/// A piece of message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// Plain text.
    Text(&'a str),
    /// A web link: the text as written and the URL to open.
    Url {
        /// As written in the message.
        text: &'a str,
        /// What to open (always `http` or `https`).
        href: String,
    },
    /// An e-mail address.
    Email {
        /// As written in the message.
        text: &'a str,
        /// `mailto:` URL.
        href: String,
    },
    /// `@<digits>`: a mention of a phone number (or LID) user.
    Mention {
        /// As written, including the `@`.
        text: &'a str,
        /// The digits after the `@`.
        user: &'a str,
    },
}

/// True for targets the app is willing to hand to the system opener.
pub fn is_safe_href(h: &str) -> bool {
    let l = h.to_ascii_lowercase();
    h.len() <= MAX_URL_LEN
        && (l.starts_with("https://") || l.starts_with("http://") || l.starts_with("mailto:"))
        && !h.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && !host.contains("..")
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
}

/// Host part of `rest` (after the scheme): up to `/`, `?`, `#`, `:`.
fn host_of(rest: &str) -> &str {
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    &rest[..end]
}

fn tld_ok(host: &str) -> bool {
    host.rsplit_once('.').is_some_and(|(label, tld)| {
        !label.is_empty() && TLDS.contains(&tld.to_ascii_lowercase().as_str())
    })
}

/// Link target for `core` if it is a web address.
fn web_href(core: &str) -> Option<String> {
    let lower = core.to_ascii_lowercase();
    let (scheme_len, add_scheme) = if lower.starts_with("https://") {
        (8, false)
    } else if lower.starts_with("http://") {
        (7, false)
    } else if lower.starts_with("www.") {
        (0, true)
    } else {
        // Bare domain: needs an allow-listed TLD.
        let host = host_of(core);
        if !(valid_host(host) && tld_ok(host)) {
            return None;
        }
        return Some(format!("https://{core}"));
    };
    let rest = &core[scheme_len..];
    let host = host_of(rest);
    let dotted = host.contains('.') || lower.starts_with("http");
    if !valid_host(host) || !dotted {
        return None;
    }
    Some(if add_scheme {
        format!("https://{core}")
    } else {
        core.to_string()
    })
}

fn email_href(core: &str) -> Option<String> {
    let (local, domain) = core.split_once('@')?;
    let ok_local = !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_alphanumeric() || "._%+-".contains(c));
    (ok_local && !domain.contains('@') && valid_host(domain) && tld_ok(domain))
        .then(|| format!("mailto:{core}"))
}

/// Splits trailing punctuation off a word: sentence punctuation and
/// unbalanced closing brackets are not part of the link.
fn trim_trailing(word: &str) -> &str {
    let mut end = word.len();
    while let Some(c) = word[..end].chars().next_back() {
        let cut = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '*' | '_' | '~' => true,
            ')' => word[..end].matches('(').count() < word[..end].matches(')').count(),
            ']' => word[..end].matches('[').count() < word[..end].matches(']').count(),
            '}' | '>' => true,
            _ => false,
        };
        if !cut {
            break;
        }
        end -= c.len_utf8();
    }
    &word[..end]
}

/// Splits `s` into plain text, links, addresses and mentions.
pub fn tokenize(s: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut i = 0;
    while i < s.len() {
        // Word = run of non-whitespace.
        while i < s.len() && s[i..].chars().next().is_some_and(char::is_whitespace) {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
        let start = i;
        while i < s.len() && !s[i..].chars().next().is_some_and(char::is_whitespace) {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
        if start == i {
            break;
        }
        let word = &s[start..i];
        // Leading opening punctuation is not part of the link.
        let lead = word
            .char_indices()
            .find(|(_, c)| {
                !matches!(
                    c,
                    '(' | '[' | '{' | '<' | '"' | '\'' | '*' | '_' | '~' | '¿' | '¡'
                )
            })
            .map_or(word.len(), |(k, _)| k);
        let core = trim_trailing(&word[lead..]);
        if core.is_empty() || core.len() > MAX_URL_LEN {
            continue;
        }
        let abs = start + lead;
        let tok = if let Some(user) = core.strip_prefix('@')
            && user.len() >= 5
            && user.bytes().all(|b| b.is_ascii_digit())
        {
            Some(Token::Mention { text: core, user })
        } else if core.contains('@') {
            email_href(core).map(|href| Token::Email { text: core, href })
        } else {
            web_href(core).map(|href| Token::Url { text: core, href })
        };
        if let Some(tok) = tok {
            if plain_from < abs {
                out.push(Token::Text(&s[plain_from..abs]));
            }
            out.push(tok);
            plain_from = abs + core.len();
        }
    }
    if plain_from < s.len() {
        out.push(Token::Text(&s[plain_from..]));
    }
    out
}

/// The first web link in `s`, for "open the link" shortcuts and previews.
pub fn first_url(s: &str) -> Option<String> {
    tokenize(s).into_iter().find_map(|t| match t {
        Token::Url { href, .. } => Some(href),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<String> {
        tokenize(s)
            .into_iter()
            .filter_map(|t| match t {
                Token::Url { href, .. } | Token::Email { href, .. } => Some(href),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn detects_schemes_www_and_bare_domains() {
        assert_eq!(
            urls("see https://example.com/a?b=1#c ok"),
            ["https://example.com/a?b=1#c"]
        );
        assert_eq!(urls("HTTP://Example.COM"), ["HTTP://Example.COM"]);
        assert_eq!(
            urls("visit www.rust-lang.org now"),
            ["https://www.rust-lang.org"]
        );
        assert_eq!(
            urls("github.com/FerroApps/Ferrochat"),
            ["https://github.com/FerroApps/Ferrochat"]
        );
        assert_eq!(urls("http://localhost:8080/x"), ["http://localhost:8080/x"]);
    }

    #[test]
    fn ignores_things_that_are_not_links() {
        assert!(urls("open main.rs and index.js").is_empty());
        assert!(urls("version 1.2.3 is out").is_empty());
        assert!(urls("a.b").is_empty());
        assert!(urls("https://").is_empty());
        assert!(urls("e.g. this").is_empty());
        assert!(urls("").is_empty());
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_link() {
        assert_eq!(urls("look: https://example.com."), ["https://example.com"]);
        assert_eq!(urls("(https://example.com/a)"), ["https://example.com/a"]);
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(language)"),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(urls("¿https://example.com?"), ["https://example.com"]);
        assert_eq!(urls("**https://example.com**"), ["https://example.com"]);
    }

    #[test]
    fn emails_and_mentions() {
        assert_eq!(
            urls("mail ana@example.com please"),
            ["mailto:ana@example.com"]
        );
        assert!(urls("not an email: a@b").is_empty());
        let toks = tokenize("hola @34600111222, ¿vienes?");
        assert!(toks.contains(&Token::Mention {
            text: "@34600111222",
            user: "34600111222"
        }));
        // Too short to be a phone number.
        assert!(
            !tokenize("@12")
                .iter()
                .any(|t| matches!(t, Token::Mention { .. }))
        );
    }

    #[test]
    fn tokens_cover_the_whole_text() {
        for s in [
            "plain",
            "a https://x.com b",
            "  https://x.com  ",
            "línea 1\nhttps://x.com/ñ\nlínea 3",
            "@34600111222 hola ana@example.com, www.a.es!",
        ] {
            let joined: String = tokenize(s)
                .iter()
                .map(|t| match t {
                    Token::Text(x) => (*x).to_string(),
                    Token::Url { text, .. }
                    | Token::Email { text, .. }
                    | Token::Mention { text, .. } => (*text).to_string(),
                })
                .collect();
            assert_eq!(joined, s);
        }
    }

    #[test]
    fn only_web_and_mail_targets_are_safe() {
        assert!(is_safe_href("https://example.com"));
        assert!(is_safe_href("mailto:a@b.com"));
        assert!(!is_safe_href("file:///etc/passwd"));
        assert!(!is_safe_href("javascript:alert(1)"));
        assert!(!is_safe_href("https://exa mple.com"));
        assert!(!is_safe_href(&format!(
            "https://{}",
            "a".repeat(MAX_URL_LEN)
        )));
        // Scheme-looking text never becomes a link.
        assert!(urls("file:///etc/passwd javascript:alert(1)").is_empty());
    }

    #[test]
    fn first_url_picks_the_first() {
        assert_eq!(
            first_url("x https://a.com y https://b.com").as_deref(),
            Some("https://a.com")
        );
        assert_eq!(first_url("nothing"), None);
    }
}
