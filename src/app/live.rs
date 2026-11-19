//! Typing indicators and presence: pure state machines (unit-tested, no
//! iced, no I/O).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::net::types::ChatState;

/// A peer's "typing…" indicator disappears if it is not refreshed within
/// this time (a lost `paused` must not leave it stuck).
pub const TYPING_TTL: Duration = Duration::from_secs(10);
/// We tell the chat we are typing again at most this often while the user
/// keeps typing.
pub const COMPOSING_RESEND: Duration = Duration::from_secs(10);
/// No keystroke for this long: send `paused`.
pub const COMPOSING_IDLE: Duration = Duration::from_secs(5);
/// Presence entries kept in memory.
pub const MAX_PRESENCE: usize = 500;

/// Who is typing where.
#[derive(Debug, Default)]
pub struct TypingTracker {
    active: HashMap<(String, String), (ChatState, Instant)>,
}

impl TypingTracker {
    /// Records a state change. `participant` is the group member (empty key
    /// for DMs). Returns true when what is shown for the chat changed.
    pub fn set(
        &mut self,
        chat: &str,
        participant: Option<&str>,
        state: ChatState,
        now: Instant,
    ) -> bool {
        let key = (
            chat.to_string(),
            participant.unwrap_or_default().to_string(),
        );
        match state {
            ChatState::Paused => self.active.remove(&key).is_some(),
            s => {
                let changed = self.active.get(&key).map(|(old, _)| *old) != Some(s);
                self.active.insert(key, (s, now));
                changed
            }
        }
    }

    /// A message from `sender` arrived: they stopped typing.
    pub fn clear_sender(&mut self, chat: &str, sender: &str) -> bool {
        let dm = self
            .active
            .remove(&(chat.to_string(), String::new()))
            .is_some();
        let grp = self
            .active
            .remove(&(chat.to_string(), sender.to_string()))
            .is_some();
        dm || grp
    }

    /// Drops indicators older than [`TYPING_TTL`]. Returns true if any went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.active.len();
        self.active
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < TYPING_TTL);
        self.active.len() != before
    }

    /// Anything active (the app only ticks while this is true).
    pub fn is_active(&self) -> bool {
        !self.active.is_empty()
    }

    /// Active participants of `chat`, in a stable order. For DMs the
    /// participant is empty.
    pub fn in_chat(&self, chat: &str) -> Vec<(String, ChatState)> {
        let mut v: Vec<(String, ChatState)> = self
            .active
            .iter()
            .filter(|((c, _), _)| c == chat)
            .map(|((_, p), (s, _))| (p.clone(), *s))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Forgets everything (logout).
    pub fn clear(&mut self) {
        self.active.clear();
    }
}

/// "typing…" / "Ana is typing…" / "Ana and 2 others are typing…" for the
/// header and chat list. `names` are display names, `group` selects the
/// prefixed form. `None` when nobody is typing.
pub fn typing_label(names: &[(String, ChatState)], group: bool) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let recording = names.iter().all(|(_, s)| *s == ChatState::Recording);
    let what = if recording {
        "recording audio…"
    } else {
        "typing…"
    };
    if !group {
        return Some(what.to_string());
    }
    let verb = if names.len() == 1 { "is" } else { "are" };
    let who = match names {
        [(a, _)] => a.clone(),
        [(a, _), (b, _)] => format!("{a} and {b}"),
        [(a, _), rest @ ..] => format!("{a} and {} others", rest.len()),
        [] => return None,
    };
    Some(format!("{who} {verb} {what}"))
}

/// Our own "composing" presence, throttled so a stream of keystrokes
/// produces a handful of stanzas: `composing` when typing starts and every
/// [`COMPOSING_RESEND`] while it continues, `paused` after
/// [`COMPOSING_IDLE`] without input, when the field empties, on send and
/// when the chat is left.
#[derive(Debug, Default)]
pub struct ComposeThrottle {
    /// Chat we last told "composing".
    chat: Option<String>,
    sent_at: Option<Instant>,
    last_input: Option<Instant>,
}

/// What to transmit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emit {
    /// Chat to tell.
    pub chat: String,
    /// State to send.
    pub state: ChatState,
}

impl ComposeThrottle {
    /// The composer changed. `non_empty` is whether it now holds text.
    pub fn on_input(&mut self, chat: &str, non_empty: bool, now: Instant) -> Option<Emit> {
        if !non_empty {
            return self.stop();
        }
        self.last_input = Some(now);
        let same_chat = self.chat.as_deref() == Some(chat);
        let fresh = self
            .sent_at
            .is_some_and(|t| now.saturating_duration_since(t) < COMPOSING_RESEND);
        if same_chat && fresh {
            return None;
        }
        // Switching chats mid-typing: close the old one first is the
        // caller's job (it calls `stop` when the chat changes).
        self.chat = Some(chat.to_string());
        self.sent_at = Some(now);
        Some(Emit {
            chat: chat.to_string(),
            state: ChatState::Composing,
        })
    }

    /// Periodic check while composing.
    pub fn on_tick(&mut self, now: Instant) -> Option<Emit> {
        let idle = self
            .last_input
            .is_some_and(|t| now.saturating_duration_since(t) >= COMPOSING_IDLE);
        if self.chat.is_some() && idle {
            return self.stop();
        }
        None
    }

    /// Sent, cleared, chat closed or recording started: say `paused`.
    pub fn stop(&mut self) -> Option<Emit> {
        let chat = self.chat.take()?;
        self.sent_at = None;
        self.last_input = None;
        Some(Emit {
            chat,
            state: ChatState::Paused,
        })
    }

    /// True while a `composing` is outstanding (the app ticks only then).
    pub fn is_active(&self) -> bool {
        self.chat.is_some()
    }
}

/// What we know about a contact's presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceInfo {
    /// Online right now.
    pub online: bool,
    /// Last seen (unix seconds), if shared.
    pub last_seen: Option<i64>,
}

/// Bounded presence map.
#[derive(Debug, Default)]
pub struct PresenceBook {
    map: HashMap<String, PresenceInfo>,
}

impl PresenceBook {
    /// Records an update. A contact going offline without a timestamp keeps
    /// the previous `last_seen` (the server often omits it) and falls back to
    /// `now` for contacts we saw online.
    pub fn update(&mut self, jid: &str, online: bool, last_seen: Option<i64>, now: i64) {
        if self.map.len() >= MAX_PRESENCE && !self.map.contains_key(jid) {
            self.map.clear();
        }
        let prev = self.map.get(jid).copied();
        let last_seen = last_seen.or_else(|| {
            if online {
                prev.and_then(|p| p.last_seen)
            } else {
                match prev {
                    Some(p) if p.online => Some(now),
                    Some(p) => p.last_seen,
                    None => None,
                }
            }
        });
        self.map
            .insert(jid.to_string(), PresenceInfo { online, last_seen });
    }

    /// Current knowledge.
    pub fn get(&self, jid: &str) -> Option<PresenceInfo> {
        self.map.get(jid).copied()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Entries held.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn typing_set_expire_and_clear() {
        let mut t = TypingTracker::default();
        let now = t0();
        assert!(t.set("a", None, ChatState::Composing, now));
        assert!(
            !t.set("a", None, ChatState::Composing, now),
            "refresh is not a change"
        );
        assert!(t.set("a", None, ChatState::Recording, now));
        assert!(t.is_active());
        assert!(!t.expire(now + Duration::from_secs(5)));
        assert!(t.expire(now + TYPING_TTL));
        assert!(!t.is_active());
        t.set("g", Some("ana"), ChatState::Composing, now);
        assert!(t.clear_sender("g", "ana"));
        assert!(!t.clear_sender("g", "ana"));
        t.set("g", Some("ana"), ChatState::Composing, now);
        assert!(t.set("g", Some("ana"), ChatState::Paused, now));
        assert!(t.in_chat("g").is_empty());
    }

    #[test]
    fn labels() {
        use ChatState::*;
        assert_eq!(typing_label(&[], false), None);
        assert_eq!(
            typing_label(&[("".into(), Composing)], false).unwrap(),
            "typing…"
        );
        assert_eq!(
            typing_label(&[("".into(), Recording)], false).unwrap(),
            "recording audio…"
        );
        assert_eq!(
            typing_label(&[("Ana".into(), Composing)], true).unwrap(),
            "Ana is typing…"
        );
        assert_eq!(
            typing_label(
                &[("Ana".into(), Composing), ("Bob".into(), Composing)],
                true
            )
            .unwrap(),
            "Ana and Bob are typing…"
        );
        assert_eq!(
            typing_label(
                &[
                    ("Ana".into(), Composing),
                    ("Bob".into(), Recording),
                    ("Cy".into(), Composing)
                ],
                true
            )
            .unwrap(),
            "Ana and 2 others are typing…"
        );
        assert_eq!(
            typing_label(&[("Ana".into(), Recording)], true).unwrap(),
            "Ana is recording audio…"
        );
    }

    #[test]
    fn composing_is_throttled() {
        let mut c = ComposeThrottle::default();
        let now = t0();
        assert_eq!(
            c.on_input("a", true, now),
            Some(Emit {
                chat: "a".into(),
                state: ChatState::Composing
            })
        );
        // A burst of keystrokes sends nothing more.
        for i in 1..50 {
            assert_eq!(
                c.on_input("a", true, now + Duration::from_millis(i * 100)),
                None
            );
        }
        // Still typing after the resend interval: one refresh.
        let later = now + COMPOSING_RESEND + Duration::from_millis(1);
        assert_eq!(
            c.on_input("a", true, later).map(|e| e.state),
            Some(ChatState::Composing)
        );
        assert_eq!(
            c.on_input("a", true, later + Duration::from_millis(10)),
            None
        );
    }

    #[test]
    fn composing_pauses_when_idle_cleared_or_stopped() {
        let mut c = ComposeThrottle::default();
        let now = t0();
        c.on_input("a", true, now);
        assert_eq!(c.on_tick(now + Duration::from_secs(2)), None);
        let e = c.on_tick(now + COMPOSING_IDLE).unwrap();
        assert_eq!((e.chat.as_str(), e.state), ("a", ChatState::Paused));
        assert!(!c.is_active());
        assert_eq!(
            c.on_tick(now + Duration::from_secs(60)),
            None,
            "paused once"
        );

        c.on_input("a", true, now);
        assert_eq!(
            c.on_input("a", false, now).map(|e| e.state),
            Some(ChatState::Paused)
        );
        assert_eq!(c.on_input("a", false, now), None);

        c.on_input("a", true, now);
        assert_eq!(c.stop().map(|e| e.state), Some(ChatState::Paused));
        assert_eq!(c.stop(), None);
    }

    #[test]
    fn presence_keeps_last_seen() {
        let mut p = PresenceBook::default();
        p.update("a", true, None, 100);
        assert_eq!(
            p.get("a"),
            Some(PresenceInfo {
                online: true,
                last_seen: None
            })
        );
        p.update("a", false, None, 200);
        assert_eq!(p.get("a").unwrap().last_seen, Some(200));
        p.update("a", false, Some(150), 300);
        assert_eq!(p.get("a").unwrap().last_seen, Some(150));
        p.update("a", false, None, 400);
        assert_eq!(
            p.get("a").unwrap().last_seen,
            Some(150),
            "unchanged when already offline"
        );
    }

    #[test]
    fn presence_is_bounded() {
        let mut p = PresenceBook::default();
        for i in 0..(MAX_PRESENCE * 2) {
            p.update(&format!("{i}"), true, None, 0);
        }
        assert!(p.len() <= MAX_PRESENCE);
    }
}
