//! Emoji picker state: categories, search and recently used emoji.
//!
//! The emoji list is the Unicode CLDR list from the `emojis` crate (default
//! skin tone only, ordered like the official picker), limited to what the
//! bundled Twemoji font draws (`ui::emoji_font`; a test shapes every entry).

use emojis::{Emoji, Group};

/// Recently used emoji kept (the official picker shows about two rows).
pub const MAX_RECENT: usize = 32;
/// Cache key the recent emoji are persisted under.
pub const RECENT_KEY: &str = "recent_emoji";
/// Newest Unicode version the bundled font covers (Twemoji 17.0).
pub const MAX_UNICODE: (u32, u32) = (17, 0);

/// Side of one emoji cell in the picker grid.
pub const CELL: f32 = 44.0;
/// Height of a section title in the picker grid.
pub const HEADER: f32 = 32.0;
/// Horizontal padding of the picker grid (both sides together).
pub const GRID_PAD: f32 = 32.0;
/// Id of the picker's scrollable (category tabs scroll it).
pub const SCROLL_ID: &str = "emoji-picker";
/// Id of the picker's search box.
pub const SEARCH_ID: &str = "emoji-search";

/// Columns of the picker grid for a conversation pane `pane_w` wide.
pub fn columns(pane_w: f32) -> usize {
    (((pane_w - GRID_PAD) / CELL).floor() as usize).clamp(6, 30)
}

/// Picker tabs, in the official order. Smileys and people share a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Recently used.
    Recent,
    /// Smileys & emotion plus people & body.
    Smileys,
    /// Animals & nature.
    Animals,
    /// Food & drink.
    Food,
    /// Activities.
    Activity,
    /// Travel & places.
    Travel,
    /// Objects.
    Objects,
    /// Symbols.
    Symbols,
    /// Flags.
    Flags,
}

impl Category {
    /// Every tab, in order.
    pub const ALL: [Category; 9] = [
        Category::Recent,
        Category::Smileys,
        Category::Animals,
        Category::Food,
        Category::Activity,
        Category::Travel,
        Category::Objects,
        Category::Symbols,
        Category::Flags,
    ];

    /// Section title.
    pub fn title(self) -> &'static str {
        match self {
            Category::Recent => "Recent",
            Category::Smileys => "Smileys & People",
            Category::Animals => "Animals & Nature",
            Category::Food => "Food & Drink",
            Category::Activity => "Activity",
            Category::Travel => "Travel & Places",
            Category::Objects => "Objects",
            Category::Symbols => "Symbols",
            Category::Flags => "Flags",
        }
    }

    /// The emoji shown on the tab.
    pub fn tab_icon(self) -> &'static str {
        match self {
            Category::Recent => "\u{1F552}",
            Category::Smileys => "\u{1F600}",
            Category::Animals => "\u{1F43B}",
            Category::Food => "\u{1F354}",
            Category::Activity => "\u{26BD}",
            Category::Travel => "\u{1F697}",
            Category::Objects => "\u{1F4A1}",
            Category::Symbols => "\u{1F523}",
            Category::Flags => "\u{1F3F3}\u{FE0F}",
        }
    }

    fn groups(self) -> &'static [Group] {
        match self {
            Category::Recent => &[],
            Category::Smileys => &[Group::SmileysAndEmotion, Group::PeopleAndBody],
            Category::Animals => &[Group::AnimalsAndNature],
            Category::Food => &[Group::FoodAndDrink],
            Category::Activity => &[Group::Activities],
            Category::Travel => &[Group::TravelAndPlaces],
            Category::Objects => &[Group::Objects],
            Category::Symbols => &[Group::Symbols],
            Category::Flags => &[Group::Flags],
        }
    }
}

/// True when the bundled font has the emoji.
pub fn supported(e: &Emoji) -> bool {
    e.unicode_version() <= emojis::UnicodeVersion::new(MAX_UNICODE.0, MAX_UNICODE.1)
}

/// The emoji of a category (not `Recent`), in picker order.
pub fn category_emoji(c: Category) -> Vec<&'static str> {
    c.groups()
        .iter()
        .flat_map(|g| g.emojis())
        .filter(|e| supported(e))
        .map(Emoji::as_str)
        .collect()
}

/// Every emoji the picker offers.
pub fn all_emoji() -> impl Iterator<Item = &'static Emoji> {
    emojis::iter().filter(|e| supported(e))
}

/// Emoji whose name or a shortcode contains every word of `query`
/// (case-insensitive), in picker order. Empty query: nothing.
pub fn search(query: &str) -> Vec<&'static str> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.trim_matches(':').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    all_emoji()
        .filter(|e| {
            let name = e.name().to_lowercase();
            words.iter().all(|w| {
                name.contains(w.as_str()) || e.shortcodes().any(|s| s.contains(w.as_str()))
            })
        })
        .map(Emoji::as_str)
        .collect()
}

/// The `'static` form of an emoji string, if the picker knows it.
pub fn intern(s: &str) -> Option<&'static str> {
    emojis::get(s).map(Emoji::as_str)
}

/// Moves `e` to the front of the recent list (bounded, no duplicates).
pub fn push_recent(recent: &mut Vec<String>, e: &str) {
    recent.retain(|r| r != e);
    recent.insert(0, e.to_string());
    recent.truncate(MAX_RECENT);
}

/// Parses the persisted recent list, dropping anything that is not a known
/// emoji (the cache is not trusted to be well-formed).
pub fn parse_recent(json: &str) -> Vec<String> {
    let list: Vec<String> = serde_json::from_str(json).unwrap_or_default();
    let mut out = Vec::new();
    for e in list {
        if intern(&e).is_some() && !out.contains(&e) {
            out.push(e);
        }
        if out.len() == MAX_RECENT {
            break;
        }
    }
    out
}

/// Picker state.
#[derive(Debug, Clone, Default)]
pub struct Picker {
    /// The search box.
    pub query: String,
    /// Recently used, newest first.
    pub recent: Vec<String>,
    /// The recent list as it was when the drawer opened: picking an emoji
    /// must not reflow the grid under the pointer (it is refreshed on the
    /// next open, like the official picker).
    pub shown_recent: Vec<String>,
}

/// One section of the picker grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Which tab it belongs to.
    pub category: Category,
    /// Emoji, in order.
    pub emoji: Vec<String>,
}

impl Picker {
    /// The drawer opened: clear the search, show the current recents.
    pub fn opened(&mut self) {
        self.query.clear();
        self.shown_recent = self.recent.clone();
    }

    /// The sections to show: search results only while searching, else
    /// "Recent" (if any) and every category.
    pub fn sections(&self) -> Vec<Section> {
        if !self.query.trim().is_empty() {
            return vec![Section {
                category: Category::Recent,
                emoji: search(&self.query).into_iter().map(String::from).collect(),
            }];
        }
        let mut out = Vec::new();
        if !self.shown_recent.is_empty() {
            out.push(Section {
                category: Category::Recent,
                emoji: self.shown_recent.clone(),
            });
        }
        for c in Category::ALL.into_iter().skip(1) {
            out.push(Section {
                category: c,
                emoji: category_emoji(c).into_iter().map(String::from).collect(),
            });
        }
        out
    }
}

/// Vertical offset of `target`'s section in a grid of `cols` columns, each
/// row `cell` high, with a `header` high title above every section.
pub fn section_offset(
    sections: &[Section],
    target: Category,
    cols: usize,
    cell: f32,
    header: f32,
) -> Option<f32> {
    let cols = cols.max(1);
    let mut y = 0.0;
    for s in sections {
        if s.category == target {
            return Some(y);
        }
        y += header + s.emoji.len().div_ceil(cols) as f32 * cell;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_cover_every_supported_emoji_once() {
        let total: usize = Category::ALL
            .into_iter()
            .map(|c| category_emoji(c).len())
            .sum();
        assert_eq!(total, all_emoji().count());
        assert!(total > 1800, "{total}");
        // Flags, keycaps, ZWJ sequences and skin-tone-able people are in.
        let all: Vec<&str> = all_emoji().map(Emoji::as_str).collect();
        for e in [
            "\u{1F1EA}\u{1F1F8}",
            "#\u{FE0F}\u{20E3}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
            "\u{1F44B}",
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        ] {
            assert!(all.contains(&e), "{e:?}");
        }
        assert!(category_emoji(Category::Recent).is_empty());
    }

    #[test]
    fn search_matches_names_and_shortcodes() {
        let r = search("thumbs");
        assert!(r.contains(&"\u{1F44D}"));
        assert!(search(":joy:").contains(&"\u{1F602}"));
        assert!(search("red heart").contains(&"\u{2764}\u{FE0F}"));
        assert!(search("   ").is_empty());
        assert!(search("zzzz-no-such-emoji").is_empty());
    }

    #[test]
    fn recent_is_bounded_and_deduplicated() {
        let mut r = Vec::new();
        for e in all_emoji().take(MAX_RECENT + 5) {
            push_recent(&mut r, e.as_str());
        }
        assert_eq!(r.len(), MAX_RECENT);
        let first = r[5].clone();
        push_recent(&mut r, &first);
        assert_eq!(r[0], first);
        assert_eq!(r.iter().filter(|e| **e == first).count(), 1);
        // Round trip through the cache, garbage dropped.
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(parse_recent(&json), r);
        assert_eq!(
            parse_recent(r#"["x","😀","😀"]"#),
            vec!["\u{1F600}".to_string()]
        );
        assert!(parse_recent("not json").is_empty());
    }

    #[test]
    fn sections_and_offsets() {
        let mut p = Picker::default();
        let s = p.sections();
        assert_eq!(s.len(), 8);
        assert_eq!(s[0].category, Category::Smileys);
        assert_eq!(
            section_offset(&s, Category::Smileys, 10, 40.0, 30.0),
            Some(0.0)
        );
        let n = s[0].emoji.len().div_ceil(10) as f32;
        assert_eq!(
            section_offset(&s, Category::Animals, 10, 40.0, 30.0),
            Some(30.0 + n * 40.0)
        );
        assert_eq!(section_offset(&s, Category::Recent, 10, 40.0, 30.0), None);
        push_recent(&mut p.recent, "\u{1F600}");
        // Not until the drawer is opened again: no reflow under the pointer.
        assert_eq!(p.sections()[0].category, Category::Smileys);
        p.opened();
        assert_eq!(p.sections()[0].category, Category::Recent);
        p.query = "pizza".into();
        let s = p.sections();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].emoji, vec!["\u{1F355}".to_string()]);
    }
}
