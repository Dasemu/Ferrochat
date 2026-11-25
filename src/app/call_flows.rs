//! App-level call flows against the fake client: ringing panel, Decline,
//! Accept, placing a call and the call bubbles they leave in the chat.

use super::Ui;
use super::call::CallUi;
use super::test_support::Harness;
use crate::call::Phase;
use crate::net::fake::{FakeOptions, Sent};
use crate::net::{CallOutcome, CallSignal, Content, IncomingCallInfo};

const MARCOS: &str = "34622222222@s.whatsapp.net";

fn ring(id: &str, video: bool) -> CallSignal {
    CallSignal::Ringing(IncomingCallInfo {
        call_id: id.into(),
        caller: MARCOS.into(),
        group: None,
        video,
        ts: chrono::Utc::now().timestamp(),
    })
}

fn call_logs(h: &Harness, chat: &str) -> Vec<(bool, crate::net::CallLog)> {
    h.app
        .store
        .load_messages(chat, None, 200)
        .unwrap()
        .into_iter()
        .filter_map(|m| match m.content {
            Content::Call(l) => Some((m.from_me, l)),
            _ => None,
        })
        .collect()
}

async fn call(h: &mut Harness, c: CallUi) {
    h.ui(Ui::Call(c)).await;
}

#[tokio::test]
async fn incoming_call_shows_the_panel_and_decline_rejects_and_logs() {
    let mut h = Harness::new().await;
    let unread = h.app.chats.get(MARCOS).unwrap().meta.unread;
    h.net(crate::net::Event::Call(ring("CALL1", false))).await;
    let c = h.app.calls.current.as_ref().expect("ringing");
    assert_eq!(c.phase, Phase::Incoming);
    assert_eq!(c.chat, MARCOS);
    // The panel is part of the view.
    let _ = crate::ui::view(&h.app);
    assert!(crate::ui::call::panel(&h.app).is_some());

    call(&mut h, CallUi::Decline).await;
    assert!(h.app.calls.current.is_none());
    assert!(h.fake.log().contains(&Sent::RejectCall("CALL1".into())));
    let logs = call_logs(&h, MARCOS);
    assert_eq!(logs.len(), 1);
    assert!(!logs[0].0, "incoming bubble");
    assert_eq!(logs[0].1.outcome, CallOutcome::Declined);
    // Declining is not unread news.
    assert_eq!(h.app.chats.get(MARCOS).unwrap().meta.unread, unread);
    // The caller's terminate afterwards adds nothing.
    h.net(crate::net::Event::Call(CallSignal::PeerEnded {
        call_id: "CALL1".into(),
    }))
    .await;
    assert_eq!(call_logs(&h, MARCOS).len(), 1);
}

#[tokio::test]
async fn unanswered_call_becomes_an_unread_missed_call_bubble() {
    let mut h = Harness::new().await;
    let before = h.app.chats.get(MARCOS).unwrap().meta.unread;
    h.net(crate::net::Event::Call(ring("CALL2", true))).await;
    h.net(crate::net::Event::Call(CallSignal::PeerEnded {
        call_id: "CALL2".into(),
    }))
    .await;
    h.net(crate::net::Event::Call(CallSignal::Missed {
        call_id: "CALL2".into(),
        caller: MARCOS.into(),
        ts: 0,
    }))
    .await;
    assert!(h.app.calls.current.is_none());
    let logs = call_logs(&h, MARCOS);
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].1.outcome, CallOutcome::Missed);
    assert!(logs[0].1.video);
    let entry = h.app.chats.get(MARCOS).unwrap();
    assert_eq!(entry.meta.unread, before + 1);
    assert_eq!(
        entry.last.as_ref().unwrap().content.preview(),
        "Missed video call"
    );
}

#[tokio::test]
async fn accepted_call_runs_and_hang_up_logs_it() {
    let mut h = Harness::new().await;
    h.net(crate::net::Event::Call(ring("CALL3", false))).await;
    call(&mut h, CallUi::Accept).await;
    assert!(h.fake.log().contains(&Sent::AcceptCall("CALL3".into())));
    assert_eq!(
        h.app.calls.current.as_ref().unwrap().phase,
        Phase::Connecting
    );
    // What the fake's media engine reports next.
    h.net(crate::net::Event::Call(CallSignal::Connected {
        call_id: "CALL3".into(),
    }))
    .await;
    assert!(matches!(
        h.app.calls.current.as_ref().unwrap().phase,
        Phase::Active(_)
    ));
    call(&mut h, CallUi::ToggleMute).await;
    assert!(h.fake.log().contains(&Sent::MuteCall {
        call_id: "CALL3".into(),
        muted: true
    }));
    call(&mut h, CallUi::HangUp).await;
    assert!(h.fake.log().contains(&Sent::EndCall("CALL3".into())));
    assert!(h.app.calls.current.is_none());
    let logs = call_logs(&h, MARCOS);
    assert_eq!(logs[0].1.outcome, CallOutcome::Answered);
}

#[tokio::test]
async fn header_call_button_places_a_voice_call() {
    let mut h = Harness::new().await;
    call(
        &mut h,
        CallUi::Start {
            chat: MARCOS.into(),
            video: false,
        },
    )
    .await;
    let placed = h.fake.log().into_iter().find_map(|s| match s {
        Sent::StartCall { peer, call_id } => Some((peer, call_id)),
        _ => None,
    });
    let (peer, id) = placed.expect("start_call");
    assert_eq!(peer, MARCOS);
    let c = h.app.calls.current.as_ref().unwrap();
    assert_eq!(c.phase, Phase::Ringing);
    assert!(!c.incoming);
    // Cancel before the peer answers: an outgoing "no answer" bubble.
    call(&mut h, CallUi::HangUp).await;
    assert!(h.fake.log().contains(&Sent::EndCall(id)));
    let logs = call_logs(&h, MARCOS);
    assert!(logs[0].0, "outgoing bubble");
    assert_eq!(logs[0].1.outcome, CallOutcome::NoAnswer);
}

#[tokio::test]
async fn video_button_explains_instead_of_pretending() {
    let mut h = Harness::new().await;
    call(
        &mut h,
        CallUi::Start {
            chat: MARCOS.into(),
            video: true,
        },
    )
    .await;
    assert!(h.app.calls.current.is_none());
    assert_eq!(h.app.toast.as_deref(), Some(super::call::VIDEO_UNSUPPORTED));
    assert!(
        !h.fake
            .log()
            .iter()
            .any(|s| matches!(s, Sent::StartCall { .. }))
    );
}

#[tokio::test]
async fn failed_call_start_toasts_and_clears() {
    let mut h = Harness::with(FakeOptions {
        auto_receipts: false,
        fail_sends: true,
        ..FakeOptions::default()
    })
    .await;
    call(
        &mut h,
        CallUi::Start {
            chat: MARCOS.into(),
            video: false,
        },
    )
    .await;
    assert!(h.app.calls.current.is_none());
    assert!(
        h.app
            .toast
            .as_deref()
            .unwrap()
            .starts_with("Couldn't start the call")
    );
}
