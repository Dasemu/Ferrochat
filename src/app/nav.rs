//! Navigation features around the chat list: the rail sections (Chats,
//! Status, Communities), the "New chat" / "New group" / "New community"
//! panels, the "Starred messages" list, starring from the message menu and
//! the log out confirmation.
//!
//! Panels replace the chat list column (like the official client); nothing
//! here opens a window of its own.

use std::collections::HashSet;
use std::sync::Arc;

use iced::Task;
use iced::widget::image::Handle;

use super::message::{Done, Message, Ui};
use super::status::StatusState;
use super::{App, blocking, now_secs, store_task};
use crate::net::social::{
    CommunityInfo, MAX_GROUP_SUBJECT, MAX_NEW_GROUP_PICK, PhoneLookup, StarChange, StarredMessage,
    parse_phone,
};
use crate::net::{ChatMeta, Event, MsgKey};
use crate::store::{MAX_STARRED, StoredStatus};

/// How long a community list stays fresh before the tab refreshes it.
pub const COMMUNITIES_TTL_SECS: i64 = 60;
/// Contact rows drawn at most (the lists are not virtualised; search reaches
/// the rest).
pub const MAX_CONTACT_ROWS: usize = 400;
/// Largest picture accepted as a group icon.
pub const MAX_ICON_BYTES: u64 = 20 * 1024 * 1024;
/// Side of the JPEG sent as a group icon.
pub const ICON_SIDE: u32 = 640;

/// A rail section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    /// Chats (the default).
    #[default]
    Chats,
    /// Status updates.
    Status,
    /// Communities.
    Communities,
}

/// A panel that replaces the chat list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// "New chat": contacts, new group, new community, phone number.
    NewChat,
    /// "Add group members".
    NewGroupPick,
    /// Group name and icon.
    NewGroupName,
    /// Community name and description.
    NewCommunity,
    /// Starred messages.
    Starred,
}

/// A contact we can start a chat with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// Phone JID.
    pub jid: String,
    /// Saved name.
    pub name: String,
}

/// State of the "message a phone number" row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PhoneState {
    /// Nothing asked.
    #[default]
    Idle,
    /// Waiting for the server.
    Checking(String),
    /// The number has no WhatsApp account.
    NotOnWhatsApp(String),
    /// The check failed.
    Failed(String),
}

/// How the community list is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Load {
    /// Never asked.
    #[default]
    Idle,
    /// Waiting for the server.
    Loading,
    /// Have it.
    Loaded,
    /// The request failed.
    Failed(String),
}

/// A group icon ready to send, with its preview.
#[derive(Clone)]
pub struct GroupIcon {
    /// JPEG to upload.
    pub jpeg: Arc<Vec<u8>>,
    /// Preview handle (round tile).
    pub preview: Handle,
}

impl std::fmt::Debug for GroupIcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GroupIcon({} B)", self.jpeg.len())
    }
}

/// Navigation state kept by the app.
#[derive(Default)]
pub struct NavState {
    /// Rail section.
    pub section: Section,
    /// Panel over the chat list.
    pub panel: Option<Panel>,
    /// Contact search box (new chat and group picker).
    pub search: String,
    /// "Message a phone number" progress.
    pub phone: PhoneState,
    /// Participants picked for a new group, in pick order.
    pub picked: Vec<String>,
    /// New group subject.
    pub group_name: String,
    /// New group icon.
    pub group_icon: Option<GroupIcon>,
    /// A group / community is being created.
    pub creating: bool,
    /// New community name.
    pub community_name: String,
    /// New community description.
    pub community_desc: String,
    /// Communities.
    pub communities: Vec<CommunityInfo>,
    /// State of the community request.
    pub communities_load: Load,
    /// When the list was fetched (unix seconds).
    pub communities_at: i64,
    /// Communities the user collapsed.
    pub collapsed: HashSet<String>,
    /// Starred messages, newest star first.
    pub starred: Vec<StarredMessage>,
    /// The starred list is loading.
    pub starred_loading: bool,
    /// `(chat, id)` of every starred message (menu label, sync).
    pub star_set: HashSet<(String, String)>,
    /// The log out confirmation is open.
    pub confirm_logout: bool,
}

/// Navigation and status actions, wrapped in [`Ui::Nav`].
#[derive(Debug, Clone)]
pub enum NavMsg {
    /// Rail icon.
    ShowSection(Section),
    /// Pencil-plus button.
    OpenNewChat,
    /// "New group" (menu or panel).
    OpenNewGroup,
    /// "New community".
    OpenNewCommunity,
    /// Back / close a panel.
    ClosePanel,
    /// Contact search edited.
    ContactSearch(String),
    /// A contact row clicked in "New chat".
    OpenContact(String),
    /// "Message +34 ..." row clicked.
    CheckPhone,
    /// A contact ticked / unticked for the new group.
    TogglePick(String),
    /// Arrow: to the name step.
    PickNext,
    /// Back to the participants.
    PickBack,
    /// Group subject edited.
    GroupName(String),
    /// Choose an icon file.
    PickIcon,
    /// Remove the icon.
    ClearIcon,
    /// Create the group.
    CreateGroup,
    /// Community name edited.
    CommunityName(String),
    /// Community description edited.
    CommunityDesc(String),
    /// Create the community.
    CreateCommunity,
    /// Starred messages list.
    OpenStarred,
    /// Star / unstar a message (id in the open chat).
    ToggleStar(String),
    /// Unstar from the starred list.
    Unstar {
        /// Chat.
        chat: String,
        /// Message id.
        id: String,
    },
    /// "Log out" in the menu: ask first.
    AskLogout,
    /// Dismiss the confirmation.
    CancelLogout,
    /// Confirmed.
    ConfirmLogout,
    /// Expand / collapse a community.
    ToggleCommunity(String),
    /// Reload the communities.
    RefreshCommunities,
    /// A subgroup row clicked.
    OpenSubgroup(String),
    // ---- status
    /// A row of the status list.
    OpenStatus(String),
    /// Click on the right side / arrow key.
    StatusNext,
    /// Click on the left side / arrow key.
    StatusPrev,
    /// Viewer clock.
    StatusTick,
    /// Pause / resume.
    StatusPause,
    /// Close the viewer.
    CloseStatus,
    /// "Add status update".
    StatusCompose,
    /// Composer text.
    StatusComposeText(String),
    /// Next background colour.
    StatusComposeColor,
    /// Next font.
    StatusComposeFont,
    /// Close the composer.
    StatusComposeCancel,
    /// Post the text status.
    StatusPost,
}

/// Results of tasks started by [`NavMsg`] handlers.
#[derive(Debug, Clone)]
pub enum NavDone {
    /// Cached statuses read at start-up.
    StatusesLoaded(Result<Vec<StoredStatus>, String>),
    /// A status picture was downloaded and decoded.
    StatusImage {
        /// Media cache key.
        key: String,
        /// Width, height, RGBA.
        result: Result<(u32, u32, Vec<u8>), String>,
    },
    /// A text status was posted.
    StatusPosted {
        /// New id or error.
        result: Result<String, String>,
        /// Our JID.
        own: String,
        /// Text posted.
        text: String,
        /// Background posted.
        background: u32,
        /// Font posted.
        font: i32,
    },
    /// Communities fetched.
    Communities(Result<Vec<CommunityInfo>, String>),
    /// A phone number was checked.
    Phone {
        /// Digits asked about.
        digits: String,
        /// Answer.
        result: Result<PhoneLookup, String>,
    },
    /// An icon file was read and prepared (`None` = cancelled).
    GroupIcon(Option<Result<PreparedIcon, String>>),
    /// A group was created.
    GroupCreated {
        /// Subject it was created with.
        subject: String,
        /// New JID or error.
        result: Result<String, String>,
    },
    /// A community was created.
    CommunityCreated(Result<String, String>),
    /// The starred list was read from the cache.
    StarredLoaded(Result<Vec<StarredMessage>, String>),
    /// Every starred key, read at start-up.
    StarKeys(Vec<(String, String)>),
    /// Sending a star failed: restore the previous state.
    StarFailed {
        /// Message.
        key: MsgKey,
        /// The state we tried to set.
        attempted: bool,
    },
}

/// When a screenshot scene may start: each target waits for the data it
/// shows (the fake delivers history, then statuses and stars).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneStage {
    /// Chats, names and lists arrived.
    History,
    /// Statuses arrived.
    Statuses,
    /// Starred messages arrived.
    Stars,
}

/// A group icon read from disk: JPEG, preview side, preview RGBA.
pub type PreparedIcon = (Arc<Vec<u8>>, u32, Vec<u8>);

/// Group name check result.
pub fn check_group_name(name: &str) -> Result<String, &'static str> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Type a group name.");
    }
    if n.chars().count() > MAX_GROUP_SUBJECT {
        return Err("The group name is too long.");
    }
    Ok(n.to_string())
}

/// Contacts matching `query` (name or phone digits), in the given order.
pub fn filter_contacts<'a>(contacts: &'a [Contact], query: &str) -> Vec<&'a Contact> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return contacts.iter().collect();
    }
    let digits: String = q.chars().filter(char::is_ascii_digit).collect();
    contacts
        .iter()
        .filter(|c| {
            c.name.to_lowercase().contains(&q)
                || (digits.len() >= 3
                    && c.jid.split('@').next().is_some_and(|n| n.contains(&digits)))
        })
        .collect()
}

/// First letter used for the alphabetical headers of the contact list.
pub fn initial(name: &str) -> char {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().next().unwrap_or(c))
        .map(|c| if c.is_alphabetic() { c } else { '#' })
        .unwrap_or('#')
}

/// Square, 640 px JPEG for a group icon plus a round RGBA preview.
pub fn prepare_icon(bytes: &[u8]) -> Result<(Vec<u8>, u32, Vec<u8>), String> {
    use std::io::Cursor;
    let img = crate::media::image::decode_limited(bytes).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    let side = w.min(h);
    let square = img
        .crop_imm((w - side) / 2, (h - side) / 2, side, side)
        .resize_exact(
            ICON_SIDE,
            ICON_SIDE,
            ::image::imageops::FilterType::Triangle,
        );
    let mut jpeg = Vec::new();
    ::image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), 85)
        .encode_image(&square.to_rgb8())
        .map_err(|e| e.to_string())?;
    let tile = crate::media::avatar::prepare(&jpeg).map_err(|e| e.to_string())?;
    Ok((jpeg, tile.side, tile.rgba))
}

impl App {
    /// Saved contacts with a phone number, sorted by name.
    pub(crate) fn contacts(&self) -> Vec<Contact> {
        let own = self.own_jid.as_deref().unwrap_or_default();
        let mut v: Vec<Contact> = self
            .chats
            .names
            .saved()
            .filter(|(jid, _)| jid.ends_with("@s.whatsapp.net") && *jid != own)
            .map(|(jid, name)| Contact {
                jid: jid.to_string(),
                name: name.to_string(),
            })
            .collect();
        v.sort_by_cached_key(|c| (c.name.to_lowercase(), c.jid.clone()));
        v
    }

    /// Resets the new chat / group / community scratch state.
    fn reset_panels(&mut self) {
        let n = &mut self.nav;
        n.search.clear();
        n.phone = PhoneState::Idle;
        n.picked.clear();
        n.group_name.clear();
        n.group_icon = None;
        n.creating = false;
        n.community_name.clear();
        n.community_desc.clear();
    }

    fn open_panel(&mut self, panel: Panel) -> Task<Message> {
        self.menu_open = false;
        self.sidebar_menu = None;
        self.settings_open = false;
        self.nav.section = Section::Chats;
        if matches!(
            panel,
            Panel::NewChat | Panel::NewGroupPick | Panel::NewCommunity
        ) {
            self.reset_panels();
        }
        self.nav.panel = Some(panel);
        match panel {
            Panel::Starred => self.load_starred(),
            Panel::NewChat | Panel::NewGroupPick => {
                let jids = self
                    .contacts()
                    .into_iter()
                    .take(24)
                    .map(|c| c.jid)
                    .collect();
                self.want_avatars(jids)
            }
            _ => Task::none(),
        }
    }

    /// Esc: closes confirmations, the status viewer / composer, then
    /// panels. True when something was closed.
    pub(super) fn nav_escape(&mut self) -> bool {
        if std::mem::take(&mut self.nav.confirm_logout) {
            return true;
        }
        if self.status_escape() {
            return true;
        }
        if let Some(p) = self.nav.panel {
            self.nav.panel = match p {
                Panel::NewGroupName => Some(Panel::NewGroupPick),
                _ => None,
            };
            return true;
        }
        false
    }

    /// Reducer for [`NavMsg`].
    pub(super) fn on_nav(&mut self, msg: NavMsg) -> Task<Message> {
        let msg = match self.on_status_msg(msg) {
            Ok(t) => return t,
            Err(m) => m,
        };
        match msg {
            NavMsg::ShowSection(s) => self.show_section(s),
            NavMsg::OpenNewChat => self.open_panel(Panel::NewChat),
            NavMsg::OpenNewGroup => self.open_panel(Panel::NewGroupPick),
            NavMsg::OpenNewCommunity => self.open_panel(Panel::NewCommunity),
            NavMsg::ClosePanel => {
                self.nav.panel = None;
                Task::none()
            }
            NavMsg::ContactSearch(q) => {
                self.nav.search = q;
                self.nav.phone = PhoneState::Idle;
                let jids = filter_contacts(&self.contacts(), &self.nav.search)
                    .into_iter()
                    .take(24)
                    .map(|c| c.jid.clone())
                    .collect();
                self.want_avatars(jids)
            }
            NavMsg::OpenContact(jid) => {
                self.nav.panel = None;
                self.nav.section = Section::Chats;
                self.on_ui(Ui::OpenChat(jid))
            }
            NavMsg::CheckPhone => self.check_phone(),
            NavMsg::TogglePick(jid) => {
                let n = &mut self.nav;
                if let Some(p) = n.picked.iter().position(|j| *j == jid) {
                    n.picked.remove(p);
                } else if n.picked.len() < MAX_NEW_GROUP_PICK {
                    n.picked.push(jid);
                } else {
                    self.toast("That is the most people you can add at once.");
                }
                Task::none()
            }
            NavMsg::PickNext => {
                if self.nav.picked.is_empty() {
                    self.toast("Add at least one participant.");
                } else {
                    self.nav.panel = Some(Panel::NewGroupName);
                }
                Task::none()
            }
            NavMsg::PickBack => {
                self.nav.panel = Some(Panel::NewGroupPick);
                Task::none()
            }
            NavMsg::GroupName(n) => {
                self.nav.group_name = n.chars().take(MAX_GROUP_SUBJECT).collect();
                Task::none()
            }
            NavMsg::PickIcon => pick_icon(),
            NavMsg::ClearIcon => {
                self.nav.group_icon = None;
                Task::none()
            }
            NavMsg::CreateGroup => self.create_group(),
            NavMsg::CommunityName(n) => {
                self.nav.community_name = n.chars().take(MAX_GROUP_SUBJECT).collect();
                Task::none()
            }
            NavMsg::CommunityDesc(d) => {
                self.nav.community_desc = d.chars().take(2048).collect();
                Task::none()
            }
            NavMsg::CreateCommunity => self.create_community(),
            NavMsg::OpenStarred => self.open_panel(Panel::Starred),
            NavMsg::ToggleStar(id) => {
                self.popup = None;
                match self.find_message(&id).map(|m| m.key()) {
                    Some(key) => {
                        let starred = !self.is_starred(&key.chat, &key.id);
                        self.set_star(key, starred)
                    }
                    None => Task::none(),
                }
            }
            NavMsg::Unstar { chat, id } => {
                let me = self.own_jid.clone().unwrap_or_default();
                let key = self
                    .nav
                    .starred
                    .iter()
                    .find(|s| s.message.chat == chat && s.message.id == id)
                    .map(|s| {
                        let mut k = s.message.key();
                        if k.from_me {
                            k.sender = me;
                        }
                        k
                    });
                match key {
                    Some(k) => self.set_star(k, false),
                    None => Task::none(),
                }
            }
            NavMsg::AskLogout => {
                self.menu_open = false;
                self.nav.confirm_logout = true;
                Task::none()
            }
            NavMsg::CancelLogout => {
                self.nav.confirm_logout = false;
                Task::none()
            }
            NavMsg::ConfirmLogout => {
                self.nav.confirm_logout = false;
                // The existing logout path: unlink, then wipe the session.
                self.on_ui(Ui::Logout)
            }
            NavMsg::ToggleCommunity(jid) => {
                if !self.nav.collapsed.remove(&jid) {
                    self.nav.collapsed.insert(jid);
                }
                Task::none()
            }
            NavMsg::RefreshCommunities => self.fetch_communities(true),
            NavMsg::OpenSubgroup(jid) => self.on_ui(Ui::OpenChat(jid)),
            // Handled by `on_status_msg`.
            _ => Task::none(),
        }
    }

    fn show_section(&mut self, s: Section) -> Task<Message> {
        self.settings_open = false;
        self.menu_open = false;
        self.sidebar_menu = None;
        self.nav.section = s;
        match s {
            Section::Chats => self.want_visible_avatars(),
            Section::Status => {
                let expire = self.expire_statuses();
                let split = self.status_split();
                let jids: Vec<String> = split
                    .recent
                    .iter()
                    .chain(split.viewed.iter())
                    .map(|g| g.author.clone())
                    .take(40)
                    .collect();
                Task::batch([expire, self.want_avatars(jids)])
            }
            Section::Communities => self.fetch_communities(false),
        }
    }

    // ------------------------------------------------------ communities

    fn fetch_communities(&mut self, force: bool) -> Task<Message> {
        let fresh = self.nav.communities_load == Load::Loaded
            && now_secs() - self.nav.communities_at < COMMUNITIES_TTL_SECS;
        if (fresh && !force) || self.nav.communities_load == Load::Loading {
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        self.nav.communities_load = Load::Loading;
        Task::perform(
            async move { client.communities().await.map_err(|e| e.to_string()) },
            |r| Message::done(Done::Nav(Box::new(NavDone::Communities(r)))),
        )
    }

    // -------------------------------------------------------- phone number

    fn check_phone(&mut self) -> Task<Message> {
        let Some(digits) = parse_phone(&self.nav.search) else {
            return Task::none();
        };
        let Some(client) = self.client.clone() else {
            self.toast("Not connected yet.");
            return Task::none();
        };
        if matches!(self.nav.phone, PhoneState::Checking(_)) {
            return Task::none();
        }
        self.nav.phone = PhoneState::Checking(digits.clone());
        Task::perform(
            async move {
                let d = digits.clone();
                let r = client
                    .check_numbers(std::slice::from_ref(&d))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|mut v| v.pop().ok_or_else(|| "no answer".to_string()));
                (digits, r)
            },
            |(digits, result)| {
                Message::done(Done::Nav(Box::new(NavDone::Phone { digits, result })))
            },
        )
    }

    // ------------------------------------------------------------ groups

    fn create_group(&mut self) -> Task<Message> {
        let subject = match check_group_name(&self.nav.group_name) {
            Ok(s) => s,
            Err(e) => {
                self.toast(e);
                return Task::none();
            }
        };
        if self.nav.picked.is_empty() || self.nav.creating {
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            self.toast("Not connected yet.");
            return Task::none();
        };
        self.nav.creating = true;
        let people = self.nav.picked.clone();
        let icon = self.nav.group_icon.as_ref().map(|i| (*i.jpeg).clone());
        let s2 = subject.clone();
        Task::perform(
            async move {
                client
                    .create_group(&s2, &people, icon)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| {
                Message::done(Done::Nav(Box::new(NavDone::GroupCreated {
                    subject: subject.clone(),
                    result,
                })))
            },
        )
    }

    fn create_community(&mut self) -> Task<Message> {
        let name = match check_group_name(&self.nav.community_name) {
            Ok(s) => s,
            Err(_) => {
                self.toast("Type a community name.");
                return Task::none();
            }
        };
        if self.nav.creating {
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            self.toast("Not connected yet.");
            return Task::none();
        };
        self.nav.creating = true;
        let desc = self.nav.community_desc.trim().to_string();
        Task::perform(
            async move {
                client
                    .create_community(&name, &desc)
                    .await
                    .map_err(|e| e.to_string())
            },
            |r| Message::done(Done::Nav(Box::new(NavDone::CommunityCreated(r)))),
        )
    }

    // ------------------------------------------------------------- stars

    /// Is this message starred?
    pub(crate) fn is_starred(&self, chat: &str, id: &str) -> bool {
        self.nav
            .star_set
            .contains(&(chat.to_string(), id.to_string()))
    }

    /// Optimistic star / unstar: update, persist, send, revert on failure.
    fn set_star(&mut self, key: MsgKey, starred: bool) -> Task<Message> {
        let pair = (key.chat.clone(), key.id.clone());
        if starred {
            self.nav.star_set.insert(pair);
        } else {
            self.nav.star_set.remove(&pair);
            self.nav
                .starred
                .retain(|s| !(s.message.chat == key.chat && s.message.id == key.id));
        }
        let ts = now_secs();
        let (c, i) = (key.chat.clone(), key.id.clone());
        let persist = store_task(&self.store, move |s| {
            s.set_star(&c, &i, starred, ts).map(|_| ())
        });
        let Some(client) = self.client.clone() else {
            return persist;
        };
        let send = Task::perform(
            async move {
                let r = client.set_starred(&key, starred).await;
                (key, r.map_err(|e| e.to_string()))
            },
            move |(key, r)| match r {
                Ok(()) => Message::done(Done::Nothing),
                Err(e) => {
                    tracing::warn!(error = %e, "star sync failed");
                    Message::done(Done::Nav(Box::new(NavDone::StarFailed {
                        key,
                        attempted: starred,
                    })))
                }
            },
        );
        Task::batch([persist, send])
    }

    fn load_starred(&mut self) -> Task<Message> {
        self.nav.starred_loading = true;
        let store = self.store.clone();
        blocking(
            move || {
                store
                    .starred_messages(MAX_STARRED)
                    .map_err(|e| e.to_string())
            },
            |r| Message::done(Done::Nav(Box::new(NavDone::StarredLoaded(r)))),
        )
    }

    pub(super) fn load_star_keys_task(&self) -> Task<Message> {
        let store = self.store.clone();
        blocking(
            move || {
                store
                    .starred_messages(MAX_STARRED)
                    .map(|v| {
                        v.into_iter()
                            .map(|s| (s.message.chat, s.message.id))
                            .collect()
                    })
                    .unwrap_or_default()
            },
            |v| Message::done(Done::Nav(Box::new(NavDone::StarKeys(v)))),
        )
    }

    /// A star changed on another device.
    pub(super) fn on_star_event(&mut self, c: StarChange) -> Task<Message> {
        let pair = (c.key.chat.clone(), c.key.id.clone());
        let changed = if c.starred {
            self.nav.star_set.insert(pair)
        } else {
            self.nav.star_set.remove(&pair)
        };
        let (chat, id, starred, ts) = (c.key.chat, c.key.id, c.starred, c.ts);
        let persist = store_task(&self.store, move |s| {
            s.set_star(&chat, &id, starred, ts).map(|_| ())
        });
        if changed && self.nav.panel == Some(Panel::Starred) {
            return persist.chain(self.load_starred());
        }
        // Screenshot mode: open the list once the last star is stored.
        if self.cfg.screenshot.is_some() && self.nav.star_set.len() >= 4 {
            let scene = self.nav_scene(SceneStage::Stars);
            if !scene.is_empty() {
                return persist.chain(Task::batch(scene));
            }
        }
        persist
    }

    // -------------------------------------------------------- completions

    /// Completion of a [`NavMsg`] task.
    pub(super) fn on_nav_done(&mut self, done: NavDone) -> Task<Message> {
        match done {
            NavDone::StatusesLoaded(r) => self.on_statuses_loaded(r),
            NavDone::StatusImage { key, result } => {
                self.on_status_image(key, result);
                Task::none()
            }
            NavDone::StatusPosted {
                result,
                own,
                text,
                background,
                font,
            } => self.on_status_posted(result, own, text, background, font),
            NavDone::Communities(r) => {
                match r {
                    Ok(list) => {
                        self.nav.communities = list;
                        self.nav.communities_load = Load::Loaded;
                        self.nav.communities_at = now_secs();
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "communities failed");
                        self.nav.communities_load = Load::Failed(
                            "Could not load your communities. Check your connection.".into(),
                        );
                    }
                }
                let jids: Vec<String> = self
                    .nav
                    .communities
                    .iter()
                    .flat_map(|c| {
                        std::iter::once(c.jid.clone()).chain(
                            c.subgroups
                                .iter()
                                .filter(|s| s.joined)
                                .map(|s| s.jid.clone()),
                        )
                    })
                    .take(40)
                    .collect();
                self.want_avatars(jids)
            }
            NavDone::Phone { digits, result } => {
                if self.nav.phone != PhoneState::Checking(digits.clone()) {
                    return Task::none();
                }
                match result {
                    Ok(PhoneLookup::OnWhatsApp { jid, .. }) => {
                        self.nav.phone = PhoneState::Idle;
                        self.nav.panel = None;
                        self.on_ui(Ui::OpenChat(jid))
                    }
                    Ok(PhoneLookup::NotOnWhatsApp) | Ok(PhoneLookup::Invalid) => {
                        self.nav.phone = PhoneState::NotOnWhatsApp(digits);
                        Task::none()
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "number check failed");
                        self.nav.phone = PhoneState::Failed(digits);
                        Task::none()
                    }
                }
            }
            NavDone::GroupIcon(None) => Task::none(),
            NavDone::GroupIcon(Some(Err(e))) => {
                tracing::warn!(error = %e, "group icon rejected");
                self.toast("That picture could not be used as a group icon.");
                Task::none()
            }
            NavDone::GroupIcon(Some(Ok((jpeg, side, rgba)))) => {
                self.nav.group_icon = Some(GroupIcon {
                    jpeg,
                    preview: Handle::from_rgba(side, side, rgba),
                });
                Task::none()
            }
            NavDone::GroupCreated { subject, result } => {
                self.nav.creating = false;
                match result {
                    Ok(jid) => {
                        let meta = ChatMeta {
                            id: jid.clone(),
                            name: Some(subject),
                            last_ts: now_secs(),
                            ..ChatMeta::default()
                        };
                        self.chats.apply_history(std::slice::from_ref(&meta), &[]);
                        let persist = store_task(&self.store, move |s| {
                            s.upsert_chats(std::slice::from_ref(&meta))
                        });
                        self.nav.panel = None;
                        self.nav.section = Section::Chats;
                        self.reset_panels();
                        Task::batch([persist, self.on_ui(Ui::OpenChat(jid))])
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "group creation failed");
                        self.toast("Could not create the group. Try again.");
                        Task::none()
                    }
                }
            }
            NavDone::CommunityCreated(r) => {
                self.nav.creating = false;
                match r {
                    Ok(_) => {
                        self.nav.panel = None;
                        self.reset_panels();
                        self.toast("Community created.");
                        self.nav.section = Section::Communities;
                        self.fetch_communities(true)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "community creation failed");
                        self.toast("Could not create the community. Try again.");
                        Task::none()
                    }
                }
            }
            NavDone::StarredLoaded(r) => {
                self.nav.starred_loading = false;
                if let Ok(v) = r {
                    self.nav.starred = v;
                }
                Task::none()
            }
            NavDone::StarKeys(keys) => {
                self.nav.star_set.extend(keys);
                Task::none()
            }
            NavDone::StarFailed { key, attempted } => {
                // Put the previous state back everywhere.
                self.toast(if attempted {
                    "Could not star the message."
                } else {
                    "Could not unstar the message."
                });
                let pair = (key.chat.clone(), key.id.clone());
                if attempted {
                    self.nav.star_set.remove(&pair);
                } else {
                    self.nav.star_set.insert(pair);
                }
                let (c, i, back) = (key.chat, key.id, !attempted);
                let ts = now_secs();
                store_task(&self.store, move |s| {
                    s.set_star(&c, &i, back, ts).map(|_| ())
                })
            }
        }
    }

    /// Protocol events that concern navigation. `Err(ev)` for the rest.
    pub(super) fn on_nav_event(&mut self, ev: Event) -> Result<Task<Message>, Event> {
        match ev {
            Event::Star(c) => Ok(self.on_star_event(c)),
            other => self.on_status_event(other),
        }
    }

    /// Screenshot mode: the scripted interaction of the navigation targets.
    pub(super) fn nav_scene(&mut self, stage: SceneStage) -> Vec<Task<Message>> {
        use crate::config::ScreenshotTarget as T;
        let Some((target, _)) = self.cfg.screenshot.clone() else {
            return Vec::new();
        };
        let wanted = match target {
            T::StatusList | T::StatusViewer => SceneStage::Statuses,
            T::Starred => SceneStage::Stars,
            T::Communities | T::NewChat | T::NewGroup | T::NewGroupName | T::HeaderMenu => {
                SceneStage::History
            }
            _ => return Vec::new(),
        };
        if stage != wanted || self.nav_scene_done {
            return Vec::new();
        }
        self.nav_scene_done = true;
        let n = |m: NavMsg| Task::done(Message::Ui(Ui::Nav(m)));
        let pick = |jid: &str| n(NavMsg::TogglePick(jid.to_string()));
        let mut v = Vec::new();
        match target {
            T::StatusList => v.push(n(NavMsg::ShowSection(Section::Status))),
            T::StatusViewer => {
                v.push(n(NavMsg::ShowSection(Section::Status)));
                v.push(n(NavMsg::OpenStatus("34622222222@s.whatsapp.net".into())));
            }
            T::Communities => v.push(n(NavMsg::ShowSection(Section::Communities))),
            T::NewChat => v.push(n(NavMsg::OpenNewChat)),
            T::NewGroup | T::NewGroupName => {
                v.push(n(NavMsg::OpenNewGroup));
                v.push(pick("34611111111@s.whatsapp.net"));
                v.push(pick("34633333333@s.whatsapp.net"));
                v.push(pick("34677777777@s.whatsapp.net"));
                if target == T::NewGroupName {
                    v.push(n(NavMsg::PickNext));
                    v.push(n(NavMsg::GroupName("Viaje a Lisboa".into())));
                }
            }
            T::HeaderMenu => v.push(Task::done(Message::Ui(Ui::ToggleMenu))),
            T::Starred => v.push(n(NavMsg::OpenStarred)),
            _ => {}
        }
        v
    }

    /// Fresh state after logout.
    pub(super) fn reset_nav(&mut self) {
        self.nav = NavState::default();
        self.status = StatusState::default();
    }
}

/// Opens the file dialog for a group icon and prepares the picture.
fn pick_icon() -> Task<Message> {
    Task::perform(
        async {
            let file = rfd::AsyncFileDialog::new()
                .set_title("Group icon")
                .add_filter("Pictures", &["jpg", "jpeg", "png", "webp"])
                .pick_file()
                .await?;
            let path = file.path().to_path_buf();
            let res = tokio::task::spawn_blocking(move || {
                // Size first: never read a huge file into memory.
                let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                if len > MAX_ICON_BYTES {
                    return Err("Pictures over 20 MB cannot be used.".to_string());
                }
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                prepare_icon(&bytes).map(|(j, s, r)| (Arc::new(j), s, r))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            Some(res)
        },
        |r| Message::done(Done::Nav(Box::new(NavDone::GroupIcon(r)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(jid: &str, name: &str) -> Contact {
        Contact {
            jid: format!("{jid}@s.whatsapp.net"),
            name: name.into(),
        }
    }

    #[test]
    fn contact_search_matches_names_and_numbers() {
        let all = vec![
            c("34611111111", "Lucía Fernández"),
            c("34622222222", "Marcos Díaz"),
            c("34633333333", "Carmen Ruiz"),
        ];
        assert_eq!(filter_contacts(&all, "").len(), 3);
        assert_eq!(filter_contacts(&all, "  LUC ")[0].name, "Lucía Fernández");
        assert_eq!(filter_contacts(&all, "622")[0].name, "Marcos Díaz");
        // Two digits are too short to be a number search.
        assert!(filter_contacts(&all, "62").is_empty());
        assert!(filter_contacts(&all, "zzz").is_empty());
    }

    #[test]
    fn alphabetical_headers() {
        assert_eq!(initial("álvaro"), 'Á');
        assert_eq!(initial("Marcos"), 'M');
        assert_eq!(initial("  😀 smile"), 'S');
        assert_eq!(initial("123"), '#');
        assert_eq!(initial(""), '#');
    }

    #[test]
    fn group_names_are_trimmed_and_bounded() {
        assert_eq!(check_group_name("  Piso  ").as_deref(), Ok("Piso"));
        assert!(check_group_name("   ").is_err());
        assert!(check_group_name(&"x".repeat(MAX_GROUP_SUBJECT + 1)).is_err());
        assert!(check_group_name(&"x".repeat(MAX_GROUP_SUBJECT)).is_ok());
    }

    #[test]
    fn icons_become_square_640_jpegs() {
        let png = crate::mock::sunset_png(300, 200);
        let (jpeg, side, rgba) = prepare_icon(&png).expect("icon");
        let img = ::image::load_from_memory(&jpeg).expect("decodes");
        assert_eq!((img.width(), img.height()), (ICON_SIDE, ICON_SIDE));
        assert_eq!(rgba.len() as u32, side * side * 4);
        assert!(prepare_icon(b"not an image").is_err());
    }
}
