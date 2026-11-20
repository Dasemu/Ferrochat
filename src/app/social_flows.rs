//! App-level flows for statuses, communities, the new chat / group panels,
//! starred messages and the log out confirmation, against the fake client.

use super::nav::{NavMsg, Panel, PhoneState, Section};
use super::status::TICK_MS;
use super::test_support::Harness;
use super::{Message, Ui};
use crate::mock::{OWN_JID, SHOWCASE_CHAT};
use crate::net::fake::{FakeOptions, Sent};
use crate::net::social::{StatusBody, StatusItem};
use crate::net::{Content, Event};

const LUCIA: &str = SHOWCASE_CHAT;
const MARCOS: &str = "34622222222@s.whatsapp.net";
const CARMEN: &str = "34633333333@s.whatsapp.net";

async fn nav(h: &mut Harness, m: NavMsg) {
    h.ui(Ui::Nav(m)).await;
}

fn views(h: &Harness) -> Vec<(String, Vec<String>)> {
    h.fake
        .log()
        .into_iter()
        .filter_map(|s| match s {
            Sent::StatusView { author, ids } => Some((author, ids)),
            _ => None,
        })
        .collect()
}

fn text_status(id: &str, author: &str, ago: i64) -> StatusItem {
    StatusItem {
        id: id.into(),
        author: author.into(),
        author_name: None,
        from_me: false,
        timestamp: chrono::Utc::now().timestamp() - ago,
        body: StatusBody::Text {
            text: "hi".into(),
            background: 0xFF00_0000,
            font: 0,
        },
    }
}

#[tokio::test]
async fn statuses_are_not_a_chat_and_are_grouped_by_author() {
    let h = Harness::new().await;
    assert!(h.app.chats.get("status@broadcast").is_none());
    assert!(
        h.app
            .chats
            .entries
            .iter()
            .all(|e| !e.meta.id.ends_with("@broadcast"))
    );
    let split = h.app.status_split();
    let recent: Vec<_> = split.recent.iter().map(|g| g.author.as_str()).collect();
    // Newest unseen first: Elena (10 min), Carmen (45 min), Lucía (1 h), Marcos (3 h), Mamá (20 h).
    assert_eq!(recent.len(), 5, "{recent:?}");
    assert_eq!(recent[1], CARMEN);
    assert_eq!(recent[2], LUCIA);
    let lucia = split.recent.iter().find(|g| g.author == LUCIA).unwrap();
    assert_eq!(lucia.items.len(), 2);
    assert!(split.viewed.is_empty());
}

#[tokio::test]
async fn a_status_message_in_the_chat_stream_never_shows_up_as_a_message() {
    let mut h = Harness::new().await;
    let before = h.app.chats.entries.len();
    h.net(Event::Status(vec![text_status("NEW1", MARCOS, 5)]))
        .await;
    assert_eq!(h.app.chats.entries.len(), before);
    assert!(h.app.status.items.iter().any(|i| i.id == "NEW1"));
    // Persisted.
    let rows = h
        .app
        .store
        .load_statuses(chrono::Utc::now().timestamp())
        .unwrap();
    assert!(rows.iter().any(|r| r.item.id == "NEW1"));
}

#[tokio::test]
async fn viewing_marks_seen_sends_a_receipt_and_moves_the_author_to_viewed() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::ShowSection(Section::Status)).await;
    nav(&mut h, NavMsg::OpenStatus(LUCIA.into())).await;
    // Starts at the first one.
    assert_eq!(
        views(&h),
        vec![(LUCIA.to_string(), vec!["ST2".to_string()])]
    );
    assert!(h.app.status.viewed.contains(&(LUCIA.into(), "ST2".into())));
    nav(&mut h, NavMsg::StatusNext).await;
    assert_eq!(views(&h).len(), 2);
    assert_eq!(views(&h)[1].1, vec!["ST3".to_string()]);
    // Seeing the same one again does not send another receipt.
    nav(&mut h, NavMsg::StatusPrev).await;
    assert_eq!(views(&h).len(), 2);
    nav(&mut h, NavMsg::CloseStatus).await;
    let split = h.app.status_split();
    assert!(split.viewed.iter().any(|g| g.author == LUCIA));
    // The seen flag survives a restart.
    let rows = h
        .app
        .store
        .load_statuses(chrono::Utc::now().timestamp())
        .unwrap();
    assert!(
        rows.iter()
            .filter(|r| r.item.author == LUCIA)
            .all(|r| r.viewed)
    );
}

#[tokio::test]
async fn no_view_receipts_when_read_receipts_are_off_but_it_is_still_marked_seen() {
    let mut h = Harness::with(FakeOptions {
        auto_receipts: false,
        read_receipts: false,
        ..FakeOptions::default()
    })
    .await;
    nav(&mut h, NavMsg::OpenStatus(CARMEN.into())).await;
    assert!(views(&h).is_empty(), "privacy setting respected");
    assert!(h.app.status.viewed.contains(&(CARMEN.into(), "ST4".into())));
}

#[tokio::test]
async fn our_own_status_is_never_receipted() {
    let mut h = Harness::new().await;
    let mut mine = text_status("MINE", OWN_JID, 30);
    mine.from_me = true;
    h.net(Event::Status(vec![mine])).await;
    assert!(h.app.status_split().mine.is_some());
    nav(&mut h, NavMsg::OpenStatus(OWN_JID.into())).await;
    assert!(h.app.status.viewer.is_some());
    assert!(views(&h).is_empty());
}

#[tokio::test]
async fn images_and_text_auto_advance_and_the_queue_ends_by_closing() {
    let mut h = Harness::new().await;
    // Marcos: one photo, then the next contact in the queue.
    nav(&mut h, NavMsg::OpenStatus(MARCOS.into())).await;
    let start = h.app.status.viewer.clone().unwrap();
    assert_eq!(start.index, 0);
    // The photo has to be downloaded before its clock starts.
    for _ in 0..40 {
        if h.app.status_ready(&h.app.current_status().unwrap()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        h.pump(iced::Task::none()).await;
    }
    let item = h.app.current_status().unwrap();
    assert!(h.app.status_ready(&item), "picture downloaded and decoded");
    let ticks = item.duration_ms() / TICK_MS;
    for _ in 0..ticks - 1 {
        nav(&mut h, NavMsg::StatusTick).await;
    }
    assert_eq!(h.app.current_status().unwrap().id, item.id, "not yet");
    nav(&mut h, NavMsg::StatusTick).await;
    assert_ne!(
        h.app.current_status().map(|i| i.id),
        Some(item.id),
        "advanced to the next status"
    );
    // Walk to the end: the viewer closes by itself.
    for _ in 0..2000 {
        if h.app.status.viewer.is_none() {
            break;
        }
        // Give pictures and videos time to be "ready".
        if let Some(cur) = h.app.current_status()
            && !h.app.status_ready(&cur)
        {
            nav(&mut h, NavMsg::StatusNext).await;
            continue;
        }
        nav(&mut h, NavMsg::StatusTick).await;
    }
    assert!(h.app.status.viewer.is_none());
}

#[tokio::test]
async fn clicking_right_and_left_navigates_and_pause_stops_the_clock() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStatus(LUCIA.into())).await;
    nav(&mut h, NavMsg::StatusPause).await;
    for _ in 0..200 {
        nav(&mut h, NavMsg::StatusTick).await;
    }
    assert_eq!(h.app.status.viewer.as_ref().unwrap().index, 0, "paused");
    nav(&mut h, NavMsg::StatusNext).await;
    assert_eq!(h.app.status.viewer.as_ref().unwrap().index, 1);
    nav(&mut h, NavMsg::StatusPrev).await;
    assert_eq!(h.app.status.viewer.as_ref().unwrap().index, 0);
    // Left at the very first status restarts it, it does not close.
    nav(&mut h, NavMsg::StatusPrev).await;
    assert!(h.app.status.viewer.is_some());
}

#[tokio::test]
async fn a_revoked_status_disappears_and_closes_the_viewer_if_on_screen() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStatus(CARMEN.into())).await;
    h.net(Event::StatusRevoked {
        author: CARMEN.into(),
        id: "ST4".into(),
    })
    .await;
    assert!(h.app.status.viewer.is_none());
    assert!(h.app.status.items.iter().all(|i| i.id != "ST4"));
    let rows = h
        .app
        .store
        .load_statuses(chrono::Utc::now().timestamp())
        .unwrap();
    assert!(rows.iter().all(|r| r.item.id != "ST4"));
}

#[tokio::test]
async fn expired_statuses_are_dropped_from_memory_and_the_cache() {
    let mut h = Harness::new().await;
    let now = chrono::Utc::now().timestamp();
    // Arrives already expired: ignored.
    h.net(Event::Status(vec![text_status(
        "OLD",
        MARCOS,
        crate::net::social::STATUS_TTL_SECS + 5,
    )]))
    .await;
    assert!(h.app.status.items.iter().all(|i| i.id != "OLD"));
    // Time passes.
    assert!(
        h.app
            .status
            .expire(now + crate::net::social::STATUS_TTL_SECS + 1)
    );
    assert!(h.app.status.items.is_empty());
}

#[tokio::test]
async fn posting_a_text_status_goes_to_the_saved_contacts_and_appears_as_mine() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::StatusCompose).await;
    nav(&mut h, NavMsg::StatusComposeText("Hola a todos".into())).await;
    nav(&mut h, NavMsg::StatusComposeColor).await;
    nav(&mut h, NavMsg::StatusPost).await;
    let posted = h.fake.log().into_iter().find_map(|s| match s {
        Sent::PostStatus { text, recipients } => Some((text, recipients)),
        _ => None,
    });
    let (text, recipients) = posted.expect("posted");
    assert_eq!(text, "Hola a todos");
    assert_eq!(recipients, h.app.contacts().len());
    assert!(recipients > 0);
    assert!(h.app.status.compose.is_none());
    assert!(h.app.status_split().mine.is_some());
}

#[tokio::test]
async fn an_empty_status_is_not_posted() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::StatusCompose).await;
    nav(&mut h, NavMsg::StatusComposeText("   ".into())).await;
    nav(&mut h, NavMsg::StatusPost).await;
    assert!(
        h.fake
            .log()
            .iter()
            .all(|s| !matches!(s, Sent::PostStatus { .. }))
    );
}

// ------------------------------------------------------------ communities

#[tokio::test]
async fn the_communities_tab_lists_communities_and_opens_a_subgroup() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::ShowSection(Section::Communities)).await;
    assert_eq!(h.app.nav.communities.len(), 2);
    let first = h.app.nav.communities[0].clone();
    assert!(first.subgroups[0].announcement, "announcement group first");
    let joined = first
        .subgroups
        .iter()
        .find(|s| s.joined && !s.announcement)
        .unwrap()
        .clone();
    nav(&mut h, NavMsg::OpenSubgroup(joined.jid.clone())).await;
    assert_eq!(h.app.open.as_ref().unwrap().chat, joined.jid);
    // Collapsing keeps the data.
    nav(&mut h, NavMsg::ToggleCommunity(first.jid.clone())).await;
    assert!(h.app.nav.collapsed.contains(&h.app.nav.communities[0].jid));
}

// -------------------------------------------------------------- new chat

#[tokio::test]
async fn new_chat_lists_saved_contacts_sorted_and_opens_one() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenNewChat).await;
    assert_eq!(h.app.nav.panel, Some(Panel::NewChat));
    let contacts = h.app.contacts();
    assert!(contacts.len() >= 5);
    let names: Vec<_> = contacts.iter().map(|c| c.name.to_lowercase()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert!(contacts.iter().all(|c| c.jid != OWN_JID));
    nav(&mut h, NavMsg::OpenContact(MARCOS.into())).await;
    assert_eq!(h.app.nav.panel, None);
    assert_eq!(h.app.open.as_ref().unwrap().chat, MARCOS);
}

#[tokio::test]
async fn messaging_a_number_checks_whatsapp_first() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenNewChat).await;
    nav(&mut h, NavMsg::ContactSearch("+34 600 555 999".into())).await;
    nav(&mut h, NavMsg::CheckPhone).await;
    assert_eq!(
        h.app.nav.phone,
        PhoneState::NotOnWhatsApp("34600555999".into())
    );
    assert!(h.app.open.is_none());
    assert_eq!(h.app.nav.panel, Some(Panel::NewChat));

    nav(&mut h, NavMsg::ContactSearch("+34 600 555 123".into())).await;
    nav(&mut h, NavMsg::CheckPhone).await;
    assert_eq!(
        h.app.open.as_ref().map(|c| c.chat.as_str()),
        Some("34600555123@s.whatsapp.net")
    );
    assert_eq!(h.app.nav.panel, None);
    let checks: Vec<_> = h
        .fake
        .log()
        .into_iter()
        .filter_map(|s| match s {
            Sent::CheckNumbers(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(checks.len(), 2, "one check per click, none while typing");
}

#[tokio::test]
async fn text_that_is_not_a_number_is_never_sent_to_the_server() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenNewChat).await;
    nav(&mut h, NavMsg::ContactSearch("marcos".into())).await;
    nav(&mut h, NavMsg::CheckPhone).await;
    assert!(
        h.fake
            .log()
            .iter()
            .all(|s| !matches!(s, Sent::CheckNumbers(_)))
    );
}

// ------------------------------------------------------------ new group

#[tokio::test]
async fn creating_a_group_picks_people_then_names_it_and_opens_it() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenNewGroup).await;
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupPick));
    // Next without anyone does nothing.
    nav(&mut h, NavMsg::PickNext).await;
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupPick));
    nav(&mut h, NavMsg::TogglePick(LUCIA.into())).await;
    nav(&mut h, NavMsg::TogglePick(MARCOS.into())).await;
    nav(&mut h, NavMsg::TogglePick(CARMEN.into())).await;
    nav(&mut h, NavMsg::TogglePick(MARCOS.into())).await;
    assert_eq!(
        h.app.nav.picked,
        vec![LUCIA.to_string(), CARMEN.to_string()]
    );
    nav(&mut h, NavMsg::PickNext).await;
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupName));
    // No name: nothing is created.
    nav(&mut h, NavMsg::CreateGroup).await;
    assert!(
        h.fake
            .log()
            .iter()
            .all(|s| !matches!(s, Sent::CreateGroup { .. }))
    );
    nav(&mut h, NavMsg::GroupName("  Viaje a Lisboa ".into())).await;
    nav(&mut h, NavMsg::CreateGroup).await;
    let created = h.fake.log().into_iter().find_map(|s| match s {
        Sent::CreateGroup {
            subject,
            participants,
            icon,
        } => Some((subject, participants, icon)),
        _ => None,
    });
    let (subject, participants, icon) = created.expect("created");
    assert_eq!(subject, "Viaje a Lisboa");
    assert_eq!(participants, vec![LUCIA.to_string(), CARMEN.to_string()]);
    assert!(!icon);
    // The new group is a chat and is open.
    let open = h.app.open.as_ref().expect("open").chat.clone();
    assert!(open.ends_with("@g.us"));
    assert_eq!(h.app.chats.display_name(&open), "Viaje a Lisboa");
    assert_eq!(h.app.nav.panel, None);
    assert!(h.app.nav.picked.is_empty());
}

#[tokio::test]
async fn a_failed_group_creation_keeps_the_form_and_tells_the_user() {
    let mut h = Harness::with(FakeOptions {
        auto_receipts: false,
        fail_sends: true,
        ..FakeOptions::default()
    })
    .await;
    nav(&mut h, NavMsg::OpenNewGroup).await;
    nav(&mut h, NavMsg::TogglePick(LUCIA.into())).await;
    nav(&mut h, NavMsg::PickNext).await;
    nav(&mut h, NavMsg::GroupName("Grupo".into())).await;
    nav(&mut h, NavMsg::CreateGroup).await;
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupName));
    assert_eq!(h.app.nav.group_name, "Grupo");
    assert!(!h.app.nav.creating, "can try again");
    assert!(h.app.toast.is_some());
}

#[tokio::test]
async fn creating_a_community_refreshes_the_communities_tab() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenNewCommunity).await;
    nav(&mut h, NavMsg::CommunityName("Mi comunidad".into())).await;
    nav(&mut h, NavMsg::CreateCommunity).await;
    assert!(
        h.fake
            .log()
            .iter()
            .any(|s| matches!(s, Sent::CreateCommunity { name } if name == "Mi comunidad"))
    );
    assert_eq!(h.app.nav.section, Section::Communities);
    assert_eq!(h.app.nav.panel, None);
}

// --------------------------------------------------------------- starred

#[tokio::test]
async fn starred_messages_from_the_phone_are_listed_newest_star_first() {
    let mut h = Harness::new().await;
    // The fake sent four stars after the history.
    assert_eq!(h.app.nav.star_set.len(), 4);
    nav(&mut h, NavMsg::OpenStarred).await;
    assert_eq!(h.app.nav.panel, Some(Panel::Starred));
    let list = &h.app.nav.starred;
    assert_eq!(list.len(), 4);
    assert!(list.windows(2).all(|w| w[0].starred_at >= w[1].starred_at));
    assert!(
        list.iter()
            .all(|s| matches!(s.message.content, Content::Text(_)))
    );
}

#[tokio::test]
async fn starring_from_the_menu_syncs_and_unstarring_reverts() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(LUCIA.into())).await;
    let id = h
        .app
        .open
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .find(|m| matches!(&m.content, Content::Text(t) if t.contains("Te paso el itinerario")))
        .map(|m| m.id.clone())
        .expect("message");
    assert!(!h.app.is_starred(LUCIA, &id));
    nav(&mut h, NavMsg::ToggleStar(id.clone())).await;
    assert!(h.app.is_starred(LUCIA, &id));
    assert!(h.fake.log().iter().any(
        |s| matches!(s, Sent::Star { key, starred: true } if key.id == id && key.chat == LUCIA)
    ));
    assert!(h.app.store.is_starred(LUCIA, &id).unwrap());
    nav(&mut h, NavMsg::ToggleStar(id.clone())).await;
    assert!(!h.app.is_starred(LUCIA, &id));
    assert!(
        h.fake
            .log()
            .iter()
            .any(|s| matches!(s, Sent::Star { key, starred: false } if key.id == id))
    );
    assert!(!h.app.store.is_starred(LUCIA, &id).unwrap());
}

#[tokio::test]
async fn a_failed_star_is_rolled_back() {
    let mut h = Harness::with(FakeOptions {
        auto_receipts: false,
        fail_sends: true,
        ..FakeOptions::default()
    })
    .await;
    h.ui(Ui::OpenChat(LUCIA.into())).await;
    let id = h.app.open.as_ref().unwrap().messages[0].id.clone();
    let before = h.app.is_starred(LUCIA, &id);
    nav(&mut h, NavMsg::ToggleStar(id.clone())).await;
    assert_eq!(h.app.is_starred(LUCIA, &id), before);
    assert_eq!(h.app.store.is_starred(LUCIA, &id).unwrap(), before);
    assert!(h.app.toast.is_some());
}

#[tokio::test]
async fn stars_from_another_device_update_the_set_and_the_open_list() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStarred).await;
    let n = h.app.nav.starred.len();
    let (chat, id) = {
        let m = h
            .app
            .store
            .load_messages(LUCIA, None, 5)
            .unwrap()
            .into_iter()
            .find(|m| !h.app.is_starred(&m.chat, &m.id))
            .expect("an unstarred message");
        (m.chat.clone(), m.id.clone())
    };
    h.fake.simulate_star(
        crate::net::MsgKey {
            chat: chat.clone(),
            id: id.clone(),
            from_me: false,
            sender: LUCIA.into(),
        },
        true,
    );
    // Deliver the event the fake queued.
    let ev = Event::Star(crate::net::StarChange {
        key: crate::net::MsgKey {
            chat: chat.clone(),
            id: id.clone(),
            from_me: false,
            sender: LUCIA.into(),
        },
        starred: true,
        ts: 1_800_000_000,
    });
    h.net(ev).await;
    assert!(h.app.is_starred(&chat, &id));
    assert_eq!(h.app.nav.starred.len(), n + 1, "the open list refreshed");
    assert_eq!(h.app.nav.starred[0].message.id, id, "newest star first");
}

#[tokio::test]
async fn unstarring_from_the_list_removes_the_row() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStarred).await;
    let first = h.app.nav.starred[0].message.clone();
    nav(
        &mut h,
        NavMsg::Unstar {
            chat: first.chat.clone(),
            id: first.id.clone(),
        },
    )
    .await;
    assert!(h.app.nav.starred.iter().all(|s| s.message.id != first.id));
    assert!(!h.app.is_starred(&first.chat, &first.id));
}

#[tokio::test]
async fn jumping_from_a_starred_message_opens_its_chat() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStarred).await;
    let target = h.app.nav.starred[0].message.clone();
    h.ui(Ui::OpenHit {
        chat: target.chat.clone(),
        id: target.id.clone(),
    })
    .await;
    assert_eq!(h.app.open.as_ref().unwrap().chat, target.chat);
}

// ------------------------------------------------------------------ menu

#[tokio::test]
async fn the_header_menu_items_open_their_pages() {
    let mut h = Harness::new().await;
    h.ui(Ui::ToggleMenu).await;
    assert!(h.app.menu_open);
    nav(&mut h, NavMsg::OpenNewGroup).await;
    assert!(!h.app.menu_open, "menu closed");
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupPick));
    h.ui(Ui::ToggleMenu).await;
    nav(&mut h, NavMsg::OpenStarred).await;
    assert_eq!(h.app.nav.panel, Some(Panel::Starred));
    h.ui(Ui::ToggleMenu).await;
    h.ui(Ui::OpenSettings).await;
    assert!(h.app.settings_open);
}

#[tokio::test]
async fn log_out_asks_first_and_cancel_changes_nothing() {
    let mut h = Harness::new().await;
    h.ui(Ui::ToggleMenu).await;
    nav(&mut h, NavMsg::AskLogout).await;
    assert!(h.app.nav.confirm_logout);
    assert!(!h.app.menu_open);
    assert!(h.fake.log().iter().all(|s| !matches!(s, Sent::Logout)));
    nav(&mut h, NavMsg::CancelLogout).await;
    assert!(!h.app.nav.confirm_logout);
    assert!(h.fake.log().iter().all(|s| !matches!(s, Sent::Logout)));
    assert!(!h.app.chats.entries.is_empty(), "nothing was wiped");
}

#[tokio::test]
async fn confirming_log_out_uses_the_existing_logout_path_and_wipes_the_session() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::AskLogout).await;
    nav(&mut h, NavMsg::ConfirmLogout).await;
    assert!(h.fake.log().iter().any(|s| matches!(s, Sent::Logout)));
    assert!(!h.app.nav.confirm_logout);
    assert!(h.app.chats.entries.is_empty());
    assert!(h.app.status.items.is_empty());
    assert!(h.app.nav.star_set.is_empty());
    assert!(h.app.store.starred_messages(10).unwrap().is_empty());
}

#[tokio::test]
async fn escape_closes_layers_in_order() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenNewGroup).await;
    nav(&mut h, NavMsg::TogglePick(LUCIA.into())).await;
    nav(&mut h, NavMsg::PickNext).await;
    nav(&mut h, NavMsg::AskLogout).await;
    h.ui(Ui::CloseChat).await;
    assert!(!h.app.nav.confirm_logout, "dialog first");
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupName));
    h.ui(Ui::CloseChat).await;
    assert_eq!(h.app.nav.panel, Some(Panel::NewGroupPick), "back one step");
    h.ui(Ui::CloseChat).await;
    assert_eq!(h.app.nav.panel, None);
    nav(&mut h, NavMsg::OpenStatus(LUCIA.into())).await;
    h.ui(Ui::CloseChat).await;
    assert!(h.app.status.viewer.is_none());
}

#[tokio::test]
async fn views_build_for_every_section_and_panel() {
    // Smoke test: the view functions run for each state without panicking.
    let mut h = Harness::new().await;
    let _ = crate::ui::view(&h.app);
    for s in [Section::Status, Section::Communities, Section::Chats] {
        nav(&mut h, NavMsg::ShowSection(s)).await;
        let _ = crate::ui::view(&h.app);
    }
    for m in [
        NavMsg::OpenNewChat,
        NavMsg::OpenNewGroup,
        NavMsg::OpenNewCommunity,
        NavMsg::OpenStarred,
        NavMsg::AskLogout,
    ] {
        nav(&mut h, m).await;
        let _ = crate::ui::view(&h.app);
    }
    nav(&mut h, NavMsg::CancelLogout).await;
    nav(&mut h, NavMsg::ShowSection(Section::Status)).await;
    nav(&mut h, NavMsg::StatusCompose).await;
    let _ = crate::ui::view(&h.app);
    nav(&mut h, NavMsg::StatusComposeCancel).await;
    for author in [LUCIA, MARCOS, CARMEN] {
        nav(&mut h, NavMsg::OpenStatus(author.into())).await;
        let _ = crate::ui::view(&h.app);
        nav(&mut h, NavMsg::CloseStatus).await;
    }
    let _ = Message::Quit;
}

#[tokio::test]
async fn a_video_status_uses_the_video_viewer_and_a_failing_one_never_blocks_the_queue() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStatus(LUCIA.into())).await;
    nav(&mut h, NavMsg::StatusNext).await; // the video
    assert_eq!(h.app.video.as_ref().map(|v| v.id.as_str()), Some("ST3"));
    // Let the download task finish; the helper may or may not exist here.
    h.pump(iced::Task::none()).await;
    for _ in 0..40 {
        if h.app
            .video
            .as_ref()
            .is_some_and(|v| matches!(v.phase, crate::app::video::VideoPhase::Failed(_)))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        h.pump(iced::Task::none()).await;
    }
    let failed = h
        .app
        .video
        .as_ref()
        .is_some_and(|v| matches!(v.phase, crate::app::video::VideoPhase::Failed(_)));
    if failed {
        // Without a working helper the viewer moves on after a still time.
        for _ in 0..(crate::net::social::STATUS_IMAGE_MS / TICK_MS) {
            nav(&mut h, NavMsg::StatusTick).await;
        }
        assert_ne!(h.app.current_status().map(|i| i.id), Some("ST3".into()));
    }
    // Closing the viewer ends the helper and leaves no video viewer behind.
    nav(&mut h, NavMsg::CloseStatus).await;
    assert!(h.app.video.is_none() && h.app.status.viewer.is_none());
}

#[tokio::test]
async fn escape_while_a_status_video_plays_leaves_no_video_viewer() {
    let mut h = Harness::new().await;
    nav(&mut h, NavMsg::OpenStatus(LUCIA.into())).await;
    nav(&mut h, NavMsg::StatusNext).await;
    h.ui(Ui::CloseChat).await;
    assert!(h.app.status.viewer.is_none());
    assert!(h.app.video.is_none());
}
