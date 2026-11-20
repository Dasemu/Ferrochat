//! Panels that replace the chat list column (new chat, new group, new
//! community, starred messages), the Communities tab and the log out
//! confirmation.

use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, column, container, mouse_area, opaque, row, scrollable, text, text_input,
};
use iced::{Alignment, Color, Length};

use super::icons::Icon;
use super::theme::{self, INTER_MEDIUM, INTER_SEMIBOLD};
use super::{El, avatar_for, ellipsize, fill, gap, icon, list_time};
use crate::app::nav::{
    Contact, Load, MAX_CONTACT_ROWS, NavMsg, Panel, PhoneState, Section, check_group_name,
    filter_contacts, initial,
};
use crate::app::{App, Message, Ui};
use crate::net::social::{CommunityInfo, MAX_GROUP_SUBJECT, SubGroup, parse_phone};

fn nav(m: NavMsg) -> Message {
    Message::Ui(Ui::Nav(m))
}

/// The column that replaces the chat list in the current section / panel,
/// or `None` for the normal chat list.
pub fn sidebar_override(app: &App) -> Option<El<'_>> {
    match app.nav.section {
        Section::Status => Some(super::status::list(app)),
        Section::Communities => Some(communities(app)),
        Section::Chats => app.nav.panel.map(|p| match p {
            Panel::NewChat => new_chat(app),
            Panel::NewGroupPick => group_pick(app),
            Panel::NewGroupName => group_name(app),
            Panel::NewCommunity => new_community(app),
            Panel::Starred => starred(app),
        }),
    }
}

/// The conversation pane in the Status / Communities tabs when no chat is
/// open (or the status composer).
pub fn conversation_override(app: &App) -> Option<El<'_>> {
    match app.nav.section {
        Section::Status => Some(if app.status.compose.is_some() {
            super::status::composer(app)
        } else {
            placeholder(
                app,
                Icon::CircleDashed,
                "Share status updates",
                "Click on a contact to view their status updates. They disappear after 24 hours.",
            )
        }),
        Section::Communities if app.open.is_none() => Some(placeholder(
            app,
            Icon::UsersRound,
            "Communities",
            "Pick a group from a community to open its chat.",
        )),
        _ => None,
    }
}

fn placeholder<'a>(app: &'a App, i: Icon, title: &'static str, body: &'static str) -> El<'a> {
    let p = app.theme.palette;
    container(
        column![
            container(icon(i, 56.0, p.text_secondary))
                .padding(26)
                .style(theme::rounded(p.bg_elevated, 60.0)),
            text(title).size(26).color(p.text_primary),
            text(body)
                .size(14)
                .color(p.text_secondary)
                .align_x(Alignment::Center)
                .width(420),
        ]
        .spacing(16)
        .align_x(Alignment::Center),
    )
    .center(Length::Fill)
    .style(theme::solid(p.bg_elevated))
    .into()
}

/// Back arrow, title and optional trailing element.
pub fn panel_header<'a>(
    app: &'a App,
    title: &'static str,
    back: Message,
    trailing: Option<El<'a>>,
) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let mut r = row![
        button(icon(Icon::ArrowLeft, t.metrics.icon, p.text_secondary))
            .padding(8)
            .style(theme::icon_button(t, false))
            .on_press(back),
        text(title)
            .size(t.metrics.text_title)
            .font(INTER_SEMIBOLD)
            .color(p.text_primary),
        fill(),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(x) = trailing {
        r = r.push(x);
    }
    r.padding(iced::Padding {
        top: 10.0,
        right: 12.0,
        bottom: 6.0,
        left: 12.0,
    })
    .into()
}

/// The rounded search field used by every panel.
pub fn search_box<'a>(
    app: &'a App,
    placeholder: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    container(
        container(
            row![
                icon(Icon::Search, 18.0, p.text_secondary),
                text_input(placeholder, value)
                    .on_input(on_input)
                    .size(t.metrics.text)
                    .padding([6, 0])
                    .style(theme::input(t)),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding([3, 16])
        .style(theme::rounded(p.bg_input, 20.0)),
    )
    .padding([4, 14])
    .into()
}

fn frame<'a>(app: &'a App, body: El<'a>) -> El<'a> {
    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::solid(app.theme.palette.bg_sidebar))
        .into()
}

/// A tappable row: leading element, title, optional subtitle, optional
/// trailing element.
fn list_row<'a>(
    app: &'a App,
    lead: El<'a>,
    title: String,
    subtitle: Option<String>,
    trailing: Option<El<'a>>,
    on: Option<Message>,
) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let mut col = column![
        text(ellipsize(&title, 36))
            .size(16)
            .color(p.text_primary)
            .wrapping(Wrapping::None)
            .shaping(text::Shaping::Advanced)
    ]
    .spacing(2);
    if let Some(s) = subtitle {
        col = col.push(
            text(ellipsize(&s, 44))
                .size(t.metrics.text_small + 0.5)
                .color(p.text_secondary)
                .wrapping(Wrapping::None)
                .shaping(text::Shaping::Advanced),
        );
    }
    let mut r = row![lead, col.width(Length::Fill)]
        .spacing(15)
        .align_y(Alignment::Center);
    if let Some(x) = trailing {
        r = r.push(x);
    }
    let b = button(container(r).padding([8, 16]))
        .padding(0)
        .width(Length::Fill)
        .style(theme::chat_row(t, false));
    match on {
        Some(m) => b.on_press(m).into(),
        None => b.into(),
    }
}

fn round_icon<'a>(app: &'a App, i: Icon, bg: Color) -> El<'a> {
    let p = app.theme.palette;
    container(icon(i, 22.0, p.accent_text))
        .center_x(app.theme.metrics.avatar)
        .center_y(app.theme.metrics.avatar)
        .style(theme::rounded(bg, app.theme.metrics.avatar / 2.0))
        .into()
}

fn section_label<'a>(app: &'a App, label: String) -> El<'a> {
    container(
        text(label)
            .size(14)
            .font(INTER_MEDIUM)
            .color(app.theme.palette.accent),
    )
    .padding([12, 20])
    .into()
}

fn contact_rows<'a>(
    app: &'a App,
    contacts: &[&Contact],
    mut pick: impl FnMut(&Contact) -> (Option<bool>, Message),
) -> Vec<El<'a>> {
    let p = app.theme.palette;
    let mut out: Vec<El<'a>> = Vec::new();
    let mut last = None;
    for c in contacts.iter().take(MAX_CONTACT_ROWS) {
        let letter = initial(&c.name);
        if last != Some(letter) {
            out.push(
                container(text(letter.to_string()).size(14).color(p.text_secondary))
                    .padding([10, 24])
                    .into(),
            );
            last = Some(letter);
        }
        let (checked, msg) = pick(c);
        let trailing: Option<El<'a>> = checked.map(|on| {
            container(if on {
                icon(Icon::Check, 14.0, p.accent_text)
            } else {
                Space::new().width(14).height(14).into()
            })
            .padding(3)
            .style(move |th: &iced::Theme| {
                let mut s = theme::rounded(if on { p.accent } else { Color::TRANSPARENT }, 5.0)(th);
                s.border.width = 2.0;
                s.border.color = if on { p.accent } else { p.text_secondary };
                s
            })
            .into()
        });
        let number = crate::net::fallback_name(&c.jid);
        out.push(list_row(
            app,
            avatar_for(app, &c.jid, false, app.theme.metrics.avatar),
            c.name.clone(),
            Some(number),
            trailing,
            Some(msg),
        ));
    }
    if contacts.len() > MAX_CONTACT_ROWS {
        out.push(
            container(
                text(format!(
                    "{} more contacts. Search to find them.",
                    contacts.len() - MAX_CONTACT_ROWS
                ))
                .size(13)
                .color(p.text_secondary),
            )
            .center_x(Length::Fill)
            .padding(16)
            .into(),
        );
    }
    out
}

fn scroll<'a>(app: &'a App, items: Vec<El<'a>>) -> El<'a> {
    scrollable(column(items))
        .direction(super::thin_scrollbar())
        .style(theme::scrollbar(&app.theme))
        .height(Length::Fill)
        .into()
}

// ---------------------------------------------------------------- new chat

fn new_chat(app: &App) -> El<'_> {
    let p = app.theme.palette;
    let contacts = app.contacts();
    let found = filter_contacts(&contacts, &app.nav.search);
    let mut items: Vec<El<'_>> = Vec::new();
    if app.nav.search.trim().is_empty() {
        items.push(list_row(
            app,
            round_icon(app, Icon::UserPlus, p.accent),
            "New group".into(),
            None,
            None,
            Some(nav(NavMsg::OpenNewGroup)),
        ));
        items.push(list_row(
            app,
            round_icon(app, Icon::UsersRound, p.accent),
            "New community".into(),
            None,
            None,
            Some(nav(NavMsg::OpenNewCommunity)),
        ));
    }
    if let Some(digits) = parse_phone(&app.nav.search) {
        let sub = match &app.nav.phone {
            PhoneState::Checking(d) if *d == digits => Some("Checking…".to_string()),
            PhoneState::NotOnWhatsApp(d) if *d == digits => {
                Some("This number is not on WhatsApp.".to_string())
            }
            PhoneState::Failed(d) if *d == digits => {
                Some("Could not check the number. Try again.".to_string())
            }
            _ => Some("Check and start a chat".to_string()),
        };
        items.push(list_row(
            app,
            round_icon(app, Icon::Phone, p.accent),
            format!("Message +{digits}"),
            sub,
            None,
            Some(nav(NavMsg::CheckPhone)),
        ));
    }
    if contacts.is_empty() {
        items.push(
            container(
                text("No contacts yet. Your phone's address book syncs after linking.")
                    .size(14)
                    .color(p.text_secondary),
            )
            .padding(24)
            .into(),
        );
    } else {
        items.push(section_label(app, "Contacts on WhatsApp".into()));
        if found.is_empty() && parse_phone(&app.nav.search).is_none() {
            items.push(
                container(text("No contacts found").size(14).color(p.text_secondary))
                    .padding(24)
                    .into(),
            );
        }
        items.extend(contact_rows(app, &found, |c| {
            (None, nav(NavMsg::OpenContact(c.jid.clone())))
        }));
    }
    frame(
        app,
        column![
            panel_header(app, "New chat", nav(NavMsg::ClosePanel), None),
            search_box(app, "Search name or number", &app.nav.search, |s| nav(
                NavMsg::ContactSearch(s)
            )),
            scroll(app, items),
        ]
        .into(),
    )
}

// ----------------------------------------------------------------- groups

fn group_pick(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let contacts = app.contacts();
    let found = filter_contacts(&contacts, &app.nav.search);
    let mut items: Vec<El<'_>> = Vec::new();
    if !app.nav.picked.is_empty() {
        // Picked people as chips, in pick order.
        let mut chips = row![].spacing(6);
        for jid in &app.nav.picked {
            let name = app.chats.plain_name(jid);
            chips = chips.push(
                button(
                    row![
                        text(ellipsize(&name, 16)).size(13.5).color(p.text_primary),
                        icon(Icon::X, 13.0, p.text_secondary)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([5, 10])
                .style(theme::chip(t, false))
                .on_press(nav(NavMsg::TogglePick(jid.clone()))),
            );
        }
        items.push(
            scrollable(container(chips).padding([6, 16]))
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(0).scroller_width(0),
                ))
                .into(),
        );
    }
    items.extend(contact_rows(app, &found, |c| {
        (
            Some(app.nav.picked.contains(&c.jid)),
            nav(NavMsg::TogglePick(c.jid.clone())),
        )
    }));
    if contacts.is_empty() {
        items.push(
            container(
                text("No contacts to add yet.")
                    .size(14)
                    .color(p.text_secondary),
            )
            .padding(24)
            .into(),
        );
    }
    let next = button(icon(Icon::ChevronRight, 26.0, p.accent_text))
        .padding(14)
        .style(theme::primary_button(t));
    let next = if app.nav.picked.is_empty() {
        next
    } else {
        next.on_press(nav(NavMsg::PickNext))
    };
    let fab = container(next)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding([14, 0]);
    frame(
        app,
        column![
            panel_header(app, "Add group members", nav(NavMsg::OpenNewChat), None),
            search_box(app, "Search name or number", &app.nav.search, |s| nav(
                NavMsg::ContactSearch(s)
            )),
            scroll(app, items),
            fab,
        ]
        .into(),
    )
}

fn group_name(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let n = &app.nav;
    let icon_el: El<'_> = match &n.group_icon {
        Some(i) => iced::widget::image(i.preview.clone())
            .width(120)
            .height(120)
            .into(),
        None => container(icon(Icon::Camera, 40.0, p.avatar_fg))
            .center_x(120)
            .center_y(120)
            .style(theme::rounded(p.avatar_bg, 60.0))
            .into(),
    };
    let icon_btn = button(icon_el)
        .padding(0)
        .style(theme::flat_button(Color::TRANSPARENT))
        .on_press(nav(NavMsg::PickIcon));
    let mut icon_col = column![
        icon_btn,
        text("Add group icon").size(13).color(p.text_secondary)
    ]
    .spacing(8)
    .align_x(Alignment::Center);
    if n.group_icon.is_some() {
        icon_col = icon_col.push(
            button(text("Remove").size(13))
                .padding([2, 6])
                .style(theme::link_button(t))
                .on_press(nav(NavMsg::ClearIcon)),
        );
    }
    let valid = check_group_name(&n.group_name).is_ok() && !n.creating;
    let create = button(icon(Icon::Check, 26.0, p.accent_text))
        .padding(14)
        .style(theme::primary_button(t));
    let create = if valid {
        create.on_press(nav(NavMsg::CreateGroup))
    } else {
        create
    };
    let people = app.nav.picked.len();
    frame(
        app,
        column![
            panel_header(app, "New group", nav(NavMsg::PickBack), None),
            container(icon_col).center_x(Length::Fill).padding([24, 0]),
            container(
                column![
                    container(
                        text_input("Group subject", &n.group_name)
                            .on_input(|s| nav(NavMsg::GroupName(s)))
                            .on_submit_maybe(valid.then(|| nav(NavMsg::CreateGroup)))
                            .size(16)
                            .padding([8, 0])
                            .style(theme::input(t)),
                    )
                    .padding([2, 12])
                    .style(theme::rounded(p.bg_input, 8.0)),
                    text(format!(
                        "{}/{}  ·  {} participant{}",
                        n.group_name.chars().count(),
                        MAX_GROUP_SUBJECT,
                        people,
                        if people == 1 { "" } else { "s" }
                    ))
                    .size(12.5)
                    .color(p.text_secondary),
                ]
                .spacing(8)
            )
            .padding([0, 24]),
            Space::new().height(Length::Fill),
            container(create)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .padding([14, 0]),
        ]
        .into(),
    )
}

fn new_community(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let n = &app.nav;
    let valid = check_group_name(&n.community_name).is_ok() && !n.creating;
    let create = button(icon(Icon::Check, 26.0, p.accent_text))
        .padding(14)
        .style(theme::primary_button(t));
    let create = if valid {
        create.on_press(nav(NavMsg::CreateCommunity))
    } else {
        create
    };
    let field = |input: El<'static>| -> El<'static> {
        container(input)
            .padding([2, 12])
            .style(theme::rounded(p.bg_input, 8.0))
            .into()
    };
    let name_input = field(
        text_input("Community name", &n.community_name)
            .on_input(|s| nav(NavMsg::CommunityName(s)))
            .size(16)
            .padding([8, 0])
            .style(theme::input(t))
            .into(),
    );
    let desc_input = field(
        text_input("Description (optional)", &n.community_desc)
            .on_input(|s| nav(NavMsg::CommunityDesc(s)))
            .size(15)
            .padding([8, 0])
            .style(theme::input(t))
            .into(),
    );
    frame(
        app,
        column![
            panel_header(app, "New community", nav(NavMsg::OpenNewChat), None),
            container(
                column![
                    container(round_icon(app, Icon::UsersRound, p.avatar_bg))
                        .center_x(Length::Fill),
                    name_input,
                    desc_input,
                    text(
                        "A community brings related groups together under one roof, with an \
                         announcement group for everyone."
                    )
                    .size(13)
                    .color(p.text_secondary),
                ]
                .spacing(14)
            )
            .padding(24),
            Space::new().height(Length::Fill),
            container(create)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .padding([14, 0]),
        ]
        .into(),
    )
}

// ---------------------------------------------------------------- starred

fn starred(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let now = chrono::Utc::now().timestamp();
    let mut items: Vec<El<'_>> = Vec::new();
    if app.nav.starred.is_empty() {
        items.push(
            container(
                column![
                    icon(Icon::Star, 40.0, p.text_secondary),
                    text(if app.nav.starred_loading {
                        "Loading…"
                    } else {
                        "No starred messages"
                    })
                    .size(16)
                    .color(p.text_primary),
                    text("Tap and hold on any message and choose Star to find it here later.")
                        .size(13.5)
                        .color(p.text_secondary)
                        .align_x(Alignment::Center),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .padding(40)
            .into(),
        );
    }
    for s in &app.nav.starred {
        let m = &s.message;
        let who = if m.from_me {
            "You".to_string()
        } else {
            app.chats.sender_name(&m.sender, m.sender_name.as_deref())
        };
        let chat_name = app.chats.display_name(&m.chat);
        let heading = if who == chat_name || !crate::net::is_group_jid(&m.chat) {
            if m.from_me {
                format!("You  ›  {chat_name}")
            } else {
                chat_name
            }
        } else {
            format!("{who}  ›  {chat_name}")
        };
        let body = column![
            row![
                text(ellipsize(&heading, 38))
                    .size(14)
                    .font(INTER_MEDIUM)
                    .color(p.accent)
                    .wrapping(Wrapping::None)
                    .shaping(text::Shaping::Advanced),
                fill(),
                text(list_time(m.timestamp, now))
                    .size(12)
                    .color(p.text_secondary),
            ]
            .align_y(Alignment::Center),
            text(ellipsize(&m.content.preview(), 90))
                .size(14.5)
                .color(p.text_primary)
                .shaping(text::Shaping::Advanced),
        ]
        .spacing(4)
        .width(Length::Fill);
        let unstar = button(icon(Icon::StarOff, 17.0, p.text_secondary))
            .padding(8)
            .style(theme::icon_button(t, false))
            .on_press(nav(NavMsg::Unstar {
                chat: m.chat.clone(),
                id: m.id.clone(),
            }));
        let inner = row![
            avatar_for(app, &m.chat, crate::net::is_group_jid(&m.chat), 40.0),
            body,
            unstar,
        ]
        .spacing(14)
        .align_y(Alignment::Center);
        items.push(
            column![
                button(container(inner).padding([10, 16]))
                    .padding(0)
                    .width(Length::Fill)
                    .style(theme::chat_row(t, false))
                    .on_press(Message::Ui(Ui::OpenHit {
                        chat: m.chat.clone(),
                        id: m.id.clone(),
                    })),
                container(Space::new().height(1))
                    .width(Length::Fill)
                    .style(theme::solid(p.border)),
            ]
            .into(),
        );
    }
    frame(
        app,
        column![
            panel_header(app, "Starred messages", nav(NavMsg::ClosePanel), None),
            scroll(app, items),
        ]
        .into(),
    )
}

// ------------------------------------------------------------ communities

fn communities(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let refresh = button(icon(Icon::RefreshCw, m.icon - 3.0, p.text_secondary))
        .padding(8)
        .style(theme::icon_button(
            t,
            app.nav.communities_load == Load::Loading,
        ))
        .on_press(nav(NavMsg::RefreshCommunities));
    let header = row![
        text("Communities")
            .size(m.text_title)
            .font(INTER_SEMIBOLD)
            .color(p.text_primary),
        fill(),
        button(icon(Icon::Plus, m.icon, p.text_secondary))
            .padding(8)
            .style(theme::icon_button(t, false))
            .on_press(nav(NavMsg::OpenNewCommunity)),
        refresh,
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .padding(iced::Padding {
        top: 10.0,
        right: 10.0,
        bottom: 6.0,
        left: 20.0,
    });
    let mut items: Vec<El<'_>> = Vec::new();
    match &app.nav.communities_load {
        Load::Idle | Load::Loading if app.nav.communities.is_empty() => items.push(
            container(
                text("Loading communities…")
                    .size(14)
                    .color(p.text_secondary),
            )
            .center_x(Length::Fill)
            .padding(40)
            .into(),
        ),
        Load::Failed(e) if app.nav.communities.is_empty() => items.push(
            container(
                column![
                    text(e.as_str()).size(14).color(p.text_secondary),
                    button(text("Try again").size(14))
                        .style(theme::link_button(t))
                        .on_press(nav(NavMsg::RefreshCommunities)),
                ]
                .spacing(8)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .padding(40)
            .into(),
        ),
        _ if app.nav.communities.is_empty() => items.push(
            container(
                column![
                    icon(Icon::UsersRound, 44.0, p.text_secondary),
                    text("No communities yet").size(16).color(p.text_primary),
                    text(
                        "Communities you join or create show up here, with their \
                         announcement group and sub-groups."
                    )
                    .size(13.5)
                    .color(p.text_secondary)
                    .align_x(Alignment::Center),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .padding(40)
            .into(),
        ),
        _ => {
            for c in &app.nav.communities {
                items.push(community_card(app, c));
            }
        }
    }
    frame(app, column![header, scroll(app, items)].into())
}

fn community_card<'a>(app: &'a App, c: &'a CommunityInfo) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let collapsed = app.nav.collapsed.contains(&c.jid);
    let members = c
        .members
        .map(|n| format!("{n} members"))
        .unwrap_or_default();
    let head = button(
        container(
            row![
                avatar_for(app, &c.jid, true, t.metrics.avatar),
                column![
                    text(ellipsize(&c.name, 34))
                        .size(16.5)
                        .font(INTER_MEDIUM)
                        .color(p.text_primary)
                        .wrapping(Wrapping::None)
                        .shaping(text::Shaping::Advanced),
                    text(members).size(13).color(p.text_secondary),
                ]
                .spacing(2)
                .width(Length::Fill),
                icon(
                    if collapsed {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronUp
                    },
                    18.0,
                    p.text_secondary
                ),
            ]
            .spacing(15)
            .align_y(Alignment::Center),
        )
        .padding([10, 16]),
    )
    .padding(0)
    .width(Length::Fill)
    .style(theme::chat_row(t, false))
    .on_press(nav(NavMsg::ToggleCommunity(c.jid.clone())));
    let mut col = column![head];
    if !collapsed {
        for sg in &c.subgroups {
            col = col.push(subgroup_row(app, sg));
        }
    }
    column![
        col,
        container(Space::new().height(1))
            .width(Length::Fill)
            .style(theme::solid(p.border))
    ]
    .into()
}

fn subgroup_row<'a>(app: &'a App, sg: &'a SubGroup) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let selected = app.open.as_ref().is_some_and(|c| c.chat == sg.jid);
    let lead: El<'a> = if sg.announcement {
        container(icon(Icon::MessageCircle, 18.0, p.accent_text))
            .center_x(40)
            .center_y(40)
            .style(theme::rounded(p.accent, 12.0))
            .into()
    } else {
        container(avatar_for(app, &sg.jid, true, 40.0)).into()
    };
    let label = if sg.announcement {
        format!("{} · announcements", sg.name)
    } else {
        sg.name.clone()
    };
    let sub = match (sg.joined, sg.members) {
        (false, _) => "You are not in this group".to_string(),
        (true, Some(n)) => format!("{n} members"),
        (true, None) => String::new(),
    };
    let body = row![
        gap(26.0),
        lead,
        column![
            text(ellipsize(&label, 36))
                .size(15)
                .color(if sg.joined {
                    p.text_primary
                } else {
                    p.text_secondary
                })
                .wrapping(Wrapping::None)
                .shaping(text::Shaping::Advanced),
            text(sub).size(12.5).color(p.text_secondary),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    let b = button(container(body).padding([7, 16]))
        .padding(0)
        .width(Length::Fill)
        .style(theme::chat_row(t, selected));
    if sg.joined {
        b.on_press(nav(NavMsg::OpenSubgroup(sg.jid.clone()))).into()
    } else {
        b.into()
    }
}

// ---------------------------------------------------------------- log out

/// "Log out?" confirmation.
pub fn logout_dialog(app: &App) -> Option<El<'_>> {
    if !app.nav.confirm_logout {
        return None;
    }
    let t = &app.theme;
    let p = t.palette;
    let btn = |label: &'static str, on: Message, danger: bool| -> El<'_> {
        button(text(label).size(14.5).font(INTER_MEDIUM).color(if danger {
            p.danger
        } else {
            p.accent
        }))
        .padding([9, 16])
        .style(theme::link_button(t))
        .on_press(on)
        .into()
    };
    let card = container(
        column![
            text("Log out?").size(18).color(p.text_primary),
            text(
                "This unlinks Ferrochat from your phone and removes the chats stored on this \
                 computer. You will need to scan the QR code again to use it."
            )
            .size(14)
            .color(p.text_secondary),
            row![
                fill(),
                btn("Cancel", nav(NavMsg::CancelLogout), false),
                btn("Log out", nav(NavMsg::ConfirmLogout), true),
            ]
            .spacing(4),
        ]
        .spacing(12),
    )
    .padding(24)
    .width(400)
    .style(move |th: &iced::Theme| {
        let mut s = theme::rounded(p.bg_elevated, 12.0)(th);
        s.shadow = iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 24.0,
        };
        s
    });
    Some(opaque(
        mouse_area(
            container(card)
                .center(Length::Fill)
                .style(theme::solid(Color::from_rgba(0.0, 0.0, 0.0, 0.55))),
        )
        .on_press(nav(NavMsg::CancelLogout)),
    ))
}
