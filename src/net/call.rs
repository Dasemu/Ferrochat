//! Calls: the signalling `whatsapp-rust` exposes, mapped to domain types,
//! and the live-call glue for [`super::whatsapp::RealClient`].
//!
//! * `<call>` stanzas arrive as the library's `Event::IncomingCall`
//!   (offer, accept, reject, terminate, ...), `Event::MissedCall` (an offer
//!   replayed from the offline queue, or a ringing call the caller gave up)
//!   and `Event::CallEndedElsewhere` (another of our devices answered or
//!   declined). [`signal_from_call`] and friends turn them into
//!   [`CallSignal`]s; nothing here builds a stanza by hand.
//! * Decline is `Client::voip().reject(&offer)`, the same `<reject>` WhatsApp
//!   Web sends. Answering and placing calls use the library's VoIP facade
//!   (`voip().accept(..)` / `voip().call(..)`), which owns the call key, the
//!   relay connection and SRTP; we only hand it 16 kHz PCM frames.
//! * Call history in chats: `callLogMesssage` messages and the
//!   `CALL_MISSED_*` history stubs become [`Content::Call`](super::Content).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::convert::bare_jid;

/// The library's PCM frame: 60 ms of 16 kHz mono audio.
pub const FRAME_SAMPLES: usize = 960;
/// Sample rate of call audio frames.
pub const SAMPLE_RATE: u32 = 16_000;
/// Ringing offers kept for Accept / Decline (oldest dropped first).
const MAX_RINGING: usize = 8;

/// How a call ended, as shown in the chat's call bubble.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallOutcome {
    /// Talked; `CallLog::seconds` is the duration.
    Answered,
    /// Incoming and never answered.
    Missed,
    /// Declined (by us for an incoming call, by the peer for an outgoing one).
    Declined,
    /// Outgoing, the peer never answered (or we hung up while it rang).
    NoAnswer,
    /// Answered or declined on another of our devices.
    Elsewhere,
    /// Could not connect.
    Failed,
}

/// One call in a chat's history (a call bubble).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallLog {
    /// Video call.
    pub video: bool,
    /// Group call.
    pub group: bool,
    /// Outcome.
    pub outcome: CallOutcome,
    /// Talk time in seconds (answered calls).
    pub seconds: u32,
}

impl fmt::Debug for CallLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CallLog({:?}, video={}, {}s)",
            self.outcome, self.video, self.seconds
        )
    }
}

impl CallLog {
    /// The bubble title, as the official client words it. `incoming` is the
    /// direction (the bubble side already shows it).
    pub fn title(&self, incoming: bool) -> String {
        let kind = match (self.group, self.video) {
            (false, false) => "voice call",
            (false, true) => "video call",
            (true, false) => "group voice call",
            (true, true) => "group video call",
        };
        match self.outcome {
            CallOutcome::Missed if incoming => format!("Missed {kind}"),
            CallOutcome::Declined if incoming => format!("Declined {kind}"),
            _ => capitalize(kind),
        }
    }

    /// The line under the title.
    pub fn detail(&self, incoming: bool) -> String {
        match self.outcome {
            CallOutcome::Answered => talk_time(self.seconds),
            CallOutcome::Missed | CallOutcome::Declined if incoming => "Click to call back".into(),
            CallOutcome::Missed | CallOutcome::NoAnswer => "No answer".into(),
            CallOutcome::Declined => "Declined".into(),
            CallOutcome::Elsewhere => "Answered on another device".into(),
            CallOutcome::Failed => "Couldn't connect".into(),
        }
    }

    /// One line for the chat list and notifications.
    pub fn preview(&self, incoming: bool) -> String {
        match self.outcome {
            CallOutcome::Answered => {
                format!("{} ({})", self.title(incoming), talk_time(self.seconds))
            }
            _ => self.title(incoming),
        }
    }

    /// Shown in red, like the official client's missed calls.
    pub fn is_missed(&self, incoming: bool) -> bool {
        incoming && matches!(self.outcome, CallOutcome::Missed | CallOutcome::Declined)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// "45 sec", "2 min", "1 hr 5 min" like the official call bubbles.
pub fn talk_time(seconds: u32) -> String {
    match seconds {
        0..60 => format!("{seconds} sec"),
        60..3600 => format!("{} min", seconds / 60),
        _ => {
            let (h, m) = (seconds / 3600, seconds % 3600 / 60);
            if m == 0 {
                format!("{h} hr")
            } else {
                format!("{h} hr {m} min")
            }
        }
    }
}

/// An incoming call that is ringing on this device.
#[derive(Clone, PartialEq, Eq)]
pub struct IncomingCallInfo {
    /// Call id (shared by every device and stanza of the call).
    pub call_id: String,
    /// The person calling (bare JID; phone number when known).
    pub caller: String,
    /// The group the call belongs to, if any.
    pub group: Option<String>,
    /// Video call.
    pub video: bool,
    /// Unix seconds.
    pub ts: i64,
}

impl IncomingCallInfo {
    /// The chat the call belongs to (group or caller).
    pub fn chat(&self) -> &str {
        self.group.as_deref().unwrap_or(&self.caller)
    }
}

impl fmt::Debug for IncomingCallInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "IncomingCall(video={}, group={})",
            self.video,
            self.group.is_some()
        )
    }
}

/// What the protocol layer reports about calls.
#[derive(Clone, PartialEq, Eq)]
pub enum CallSignal {
    /// Somebody is calling us.
    Ringing(IncomingCallInfo),
    /// A call that rang without being answered here: the caller gave up,
    /// or the offer was replayed from the offline queue.
    Missed {
        /// Call id.
        call_id: String,
        /// Caller (bare JID).
        caller: String,
        /// Unix seconds.
        ts: i64,
    },
    /// Another of our devices answered (`accepted`) or declined the call.
    Elsewhere {
        /// Call id.
        call_id: String,
        /// Answered (true) or declined (false).
        accepted: bool,
    },
    /// The peer answered our call.
    PeerAccepted {
        /// Call id.
        call_id: String,
    },
    /// The peer declined our call. `busy`: that device is in another call.
    PeerRejected {
        /// Call id.
        call_id: String,
        /// Busy rather than declined.
        busy: bool,
    },
    /// The peer hung up or cancelled.
    PeerEnded {
        /// Call id.
        call_id: String,
    },
    /// Media is flowing.
    Connected {
        /// Call id.
        call_id: String,
    },
    /// The call's media task stopped (hang-up, relay lost, failure).
    MediaEnded {
        /// Call id.
        call_id: String,
        /// User-facing reason when it failed.
        error: Option<String>,
    },
}

impl CallSignal {
    /// The call this signal is about.
    pub fn call_id(&self) -> &str {
        match self {
            CallSignal::Ringing(i) => &i.call_id,
            CallSignal::Missed { call_id, .. }
            | CallSignal::Elsewhere { call_id, .. }
            | CallSignal::PeerAccepted { call_id }
            | CallSignal::PeerRejected { call_id, .. }
            | CallSignal::PeerEnded { call_id }
            | CallSignal::Connected { call_id }
            | CallSignal::MediaEnded { call_id, .. } => call_id,
        }
    }
}

impl fmt::Debug for CallSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No JIDs.
        match self {
            CallSignal::Ringing(i) => write!(f, "Ringing({i:?})"),
            CallSignal::Missed { .. } => write!(f, "Missed"),
            CallSignal::Elsewhere { accepted, .. } => write!(f, "Elsewhere(accepted={accepted})"),
            CallSignal::PeerAccepted { .. } => write!(f, "PeerAccepted"),
            CallSignal::PeerRejected { busy, .. } => write!(f, "PeerRejected(busy={busy})"),
            CallSignal::PeerEnded { .. } => write!(f, "PeerEnded"),
            CallSignal::Connected { .. } => write!(f, "Connected"),
            CallSignal::MediaEnded { error, .. } => {
                write!(f, "MediaEnded(failed={})", error.is_some())
            }
        }
    }
}

/// Microphone and speaker ends of a call, as the library's VoIP facade
/// takes them: 960-sample (60 ms) mono i16 frames at 16 kHz both ways. A
/// closed microphone channel does not end the call.
pub struct CallAudio {
    /// Frames to send.
    pub mic: async_channel::Receiver<Vec<i16>>,
    /// Frames received.
    pub speaker: async_channel::Sender<Vec<i16>>,
}

impl CallAudio {
    /// No devices: nothing is sent, received frames are dropped. Used by
    /// `--mock` and the tests (no sound card) and when a device fails.
    pub fn silent() -> Self {
        let (_tx, mic) = async_channel::bounded(1);
        let (speaker, _rx) = async_channel::bounded(1);
        Self { mic, speaker }
    }
}

impl fmt::Debug for CallAudio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallAudio")
    }
}

// ------------------------------------------------------------ library mapping

use whatsapp_rust::wacore::types::call::{
    CallAction, CallEndedElsewhere, ElsewhereOutcome, IncomingCall, MissedCall,
};

/// Maps one `<call>` stanza to a signal. `None` for the transport-level
/// actions (transport, relay latency, video state, group roster, ...) the
/// library handles itself.
///
/// A `<terminate>` that reports another of our devices (`accepted_elsewhere`
/// / `rejected_elsewhere`) is left to `CallEndedElsewhere`, which the
/// library emits for the same stanza.
pub fn signal_from_call(call: &IncomingCall) -> Option<CallSignal> {
    let call_id = call.action.call_id().to_string();
    if call_id.is_empty() {
        return None;
    }
    match &call.action {
        CallAction::Offer {
            call_creator,
            caller_pn,
            is_video,
            group_jid,
            ..
        } => {
            let group = group_jid
                .as_ref()
                .map(|g| bare_jid(&g.to_string()))
                .or_else(|| call.group.as_ref().map(|_| String::new()))
                .filter(|g| !g.is_empty());
            let caller = caller_pn
                .as_ref()
                .unwrap_or(if group.is_some() {
                    call_creator
                } else {
                    &call.from
                })
                .to_string();
            Some(CallSignal::Ringing(IncomingCallInfo {
                call_id,
                caller: bare_jid(&caller),
                group,
                video: *is_video,
                ts: call.timestamp.timestamp(),
            }))
        }
        CallAction::OfferNotice {
            call_creator,
            is_video,
            is_group,
            ..
        } => Some(CallSignal::Ringing(IncomingCallInfo {
            call_id,
            caller: bare_jid(&call_creator.to_string()),
            group: is_group.then(|| bare_jid(&call.from.to_string())),
            video: *is_video,
            ts: call.timestamp.timestamp(),
        })),
        CallAction::Accept { .. } => Some(CallSignal::PeerAccepted { call_id }),
        CallAction::Reject { reason, .. } => Some(CallSignal::PeerRejected {
            call_id,
            busy: reason.as_deref() == Some("busy"),
        }),
        CallAction::Terminate { reason, .. } => match reason.as_deref() {
            Some("accepted_elsewhere" | "rejected_elsewhere") => None,
            _ => Some(CallSignal::PeerEnded { call_id }),
        },
        _ => None,
    }
}

/// `Event::MissedCall`.
pub fn signal_from_missed(m: &MissedCall) -> CallSignal {
    CallSignal::Missed {
        call_id: m.call_id.clone(),
        caller: bare_jid(&m.from.to_string()),
        ts: m.timestamp.timestamp(),
    }
}

/// `Event::CallEndedElsewhere`.
pub fn signal_from_elsewhere(e: &CallEndedElsewhere) -> CallSignal {
    CallSignal::Elsewhere {
        call_id: e.call_id.clone(),
        accepted: e.outcome == ElsewhereOutcome::Accepted,
    }
}

// ---------------------------------------------------------------- history

use whatsapp_rust::waproto::whatsapp as wa;

/// `callLogMesssage` (sic, the proto's spelling): a call the phone logged in
/// the chat.
pub fn log_from_message(c: &wa::message::CallLogMessage) -> CallLog {
    use wa::message::call_log_message::{CallOutcome as O, CallType};
    let outcome = match c.call_outcome {
        Some(O::CONNECTED) => CallOutcome::Answered,
        Some(O::REJECTED) => CallOutcome::Declined,
        Some(O::ACCEPTED_ELSEWHERE) => CallOutcome::Elsewhere,
        Some(O::FAILED) => CallOutcome::Failed,
        // Ongoing calls are logged again when they end.
        Some(O::ONGOING) => CallOutcome::Answered,
        _ => CallOutcome::Missed,
    };
    CallLog {
        video: c.is_video.unwrap_or(false),
        group: c.participants.len() > 1 || matches!(c.call_type, Some(CallType::VOICE_CHAT)),
        outcome,
        seconds: c
            .duration_secs
            .unwrap_or_default()
            .clamp(0, i64::from(u32::MAX)) as u32,
    }
}

/// History-sync stubs for missed calls (`CALL_MISSED_VOICE` and friends).
pub fn log_from_stub(stub: wa::web_message_info::StubType) -> Option<CallLog> {
    use wa::web_message_info::StubType as S;
    let (video, group) = match stub {
        S::CALL_MISSED_VOICE => (false, false),
        S::CALL_MISSED_VIDEO => (true, false),
        S::CALL_MISSED_GROUP_VOICE => (false, true),
        S::CALL_MISSED_GROUP_VIDEO => (true, true),
        _ => return None,
    };
    Some(CallLog {
        video,
        group,
        outcome: CallOutcome::Missed,
        seconds: 0,
    })
}

// ------------------------------------------------------------- live glue

use whatsapp_rust::Client;
use whatsapp_rust::voip::{CallEvent, CallHandle};

use super::Event;
use crate::error::{Error, Result};

/// The ringing offers and live call handles of the real client.
pub(super) struct CallBook {
    /// Offers ringing here, kept for Accept / Decline (the library needs the
    /// offer itself: its call key and relay).
    ringing: Mutex<Vec<IncomingCall>>,
    /// Calls with media (answered here, or placed from here).
    active: Arc<Mutex<HashMap<String, Arc<CallHandle>>>>,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
}

impl CallBook {
    pub(super) fn new(tx: tokio::sync::mpsc::UnboundedSender<Event>) -> Self {
        Self {
            ringing: Mutex::new(Vec::new()),
            active: Arc::new(Mutex::new(HashMap::new())),
            tx,
        }
    }

    /// Keeps a ringing offer and drops it when the call is over.
    pub(super) fn observe(&self, call: &IncomingCall, signal: Option<&CallSignal>) {
        let Ok(mut ringing) = self.ringing.lock() else {
            return;
        };
        match signal {
            Some(CallSignal::Ringing(_)) if matches!(call.action, CallAction::Offer { .. }) => {
                ringing.retain(|c| c.action.call_id() != call.action.call_id());
                if ringing.len() >= MAX_RINGING {
                    ringing.remove(0);
                }
                ringing.push(call.clone());
            }
            Some(CallSignal::Ringing(_)) | None => {}
            Some(s) => {
                let id = s.call_id();
                ringing.retain(|c| c.action.call_id() != id);
            }
        }
    }

    /// Forgets a ringing offer (missed / answered elsewhere).
    pub(super) fn forget(&self, call_id: &str) {
        if let Ok(mut r) = self.ringing.lock() {
            r.retain(|c| c.action.call_id() != call_id);
        }
    }

    fn take_ringing(&self, call_id: &str) -> Result<IncomingCall> {
        let mut r = self
            .ringing
            .lock()
            .map_err(|_| Error::Protocol("call state poisoned".into()))?;
        let pos = r
            .iter()
            .position(|c| c.action.call_id() == call_id)
            .ok_or_else(|| Error::Protocol("the call is no longer ringing".into()))?;
        Ok(r.remove(pos))
    }

    /// Decline: the library's `<reject>` (what WhatsApp Web sends).
    pub(super) async fn reject(&self, client: &Client, call_id: &str) -> Result<()> {
        let offer = self.take_ringing(call_id)?;
        client
            .voip()
            .reject(&offer)
            .await
            .map_err(|e| Error::Protocol(format!("decline failed: {e}")))
    }

    /// Answer: `<preaccept>`, call-key decrypt, `<accept>`, relay connect.
    pub(super) async fn accept(
        &self,
        client: &Client,
        call_id: &str,
        audio: CallAudio,
    ) -> Result<()> {
        let offer = self.take_ringing(call_id)?;
        let handle = client
            .voip()
            .accept(&offer)
            .audio(audio.mic, audio.speaker)
            .start()
            .await
            .map_err(|e| Error::Protocol(format!("could not answer: {e}")))?;
        self.track(call_id.to_string(), handle, true);
        Ok(())
    }

    /// Places a 1:1 voice call; returns its call id.
    pub(super) async fn start(
        &self,
        client: &Client,
        peer: &whatsapp_rust::Jid,
        audio: CallAudio,
    ) -> Result<String> {
        let handle = client
            .voip()
            .call(peer)
            .audio(audio.mic, audio.speaker)
            .start()
            .await
            .map_err(|e| Error::Protocol(format!("could not call: {e}")))?;
        let id = handle.call_id().to_string();
        self.track(id.clone(), handle, false);
        Ok(id)
    }

    /// Hangs up (or cancels a call that is still ringing at the peer).
    pub(super) async fn end(&self, client: &Client, call_id: &str) -> Result<()> {
        let handle = self
            .active
            .lock()
            .ok()
            .and_then(|a| a.get(call_id).cloned());
        let Some(h) = handle else {
            return Ok(());
        };
        let res = client
            .voip()
            .terminate(call_id, &h.peer_jid(), h.call_creator())
            .await;
        // Also tears down a dormant outgoing call (no engine yet).
        h.hangup().await;
        res.map_err(|e| Error::Protocol(format!("hang up failed: {e}")))
    }

    pub(super) fn set_muted(&self, call_id: &str, muted: bool) {
        if let Some(h) = self
            .active
            .lock()
            .ok()
            .and_then(|a| a.get(call_id).cloned())
        {
            h.set_muted(muted);
        }
    }

    /// Watches a call's engine events until it ends.
    fn track(&self, call_id: String, handle: CallHandle, connected: bool) {
        let handle = Arc::new(handle);
        if let Ok(mut a) = self.active.lock() {
            a.insert(call_id.clone(), handle.clone());
        }
        let tx = self.tx.clone();
        if connected {
            let _ = tx.send(Event::Call(CallSignal::Connected {
                call_id: call_id.clone(),
            }));
        }
        let events = handle.events();
        let active = self.active.clone();
        tokio::spawn(async move {
            let mut error = None;
            loop {
                tokio::select! {
                    ev = events.recv() => match ev {
                        Ok(CallEvent::RelayAllocated) => {
                            let _ = tx.send(Event::Call(CallSignal::Connected {
                                call_id: call_id.clone(),
                            }));
                        }
                        Ok(CallEvent::RelayAllocateFailed(_))
                        | Ok(CallEvent::RelayAllocateTimedOut)
                        | Ok(CallEvent::RelayReconnectTimedOut) => {
                            error = Some("Couldn't connect the call.".to_string());
                        }
                        Ok(CallEvent::AudioFormatMismatch { .. }) => {
                            error = Some("The other phone uses an audio format Ferrochat cannot play.".to_string());
                        }
                        Ok(_) => {}
                        // No more events: wait for the end below.
                        Err(_) => {
                            handle.wait_ended().await;
                            break;
                        }
                    },
                    () = handle.wait_ended() => break,
                }
            }
            if let Ok(mut a) = active.lock() {
                a.remove(&call_id);
            }
            let _ = tx.send(Event::Call(CallSignal::MediaEnded { call_id, error }));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatsapp_rust::Jid;

    fn jid(s: &str) -> Jid {
        s.parse().unwrap()
    }

    fn call(action: CallAction) -> IncomingCall {
        let ts = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        IncomingCall::new_for_test(jid("111222:3@lid"), "stanza".into(), ts, action)
    }

    fn offer(video: bool, pn: Option<&str>, group: Option<&str>) -> CallAction {
        CallAction::Offer {
            call_id: "C1".into(),
            call_creator: jid("111222@lid"),
            caller_pn: pn.map(jid),
            caller_country_code: None,
            device_class: None,
            joinable: false,
            is_video: video,
            audio: Vec::new(),
            group_jid: group.map(jid),
        }
    }

    #[test]
    fn offer_rings_under_the_phone_number() {
        let s = signal_from_call(&call(offer(true, Some("34600111222@s.whatsapp.net"), None)));
        let Some(CallSignal::Ringing(i)) = s else {
            panic!("{s:?}");
        };
        assert_eq!(i.call_id, "C1");
        assert_eq!(i.caller, "34600111222@s.whatsapp.net");
        assert_eq!(i.chat(), "34600111222@s.whatsapp.net");
        assert!(i.video);
        assert_eq!(i.ts, 1_700_000_000);
    }

    #[test]
    fn offer_without_pn_uses_the_bare_sender_and_groups_their_jid() {
        let Some(CallSignal::Ringing(i)) = signal_from_call(&call(offer(false, None, None))) else {
            panic!();
        };
        assert_eq!(i.caller, "111222@lid");
        let Some(CallSignal::Ringing(g)) =
            signal_from_call(&call(offer(false, None, Some("123-456@g.us"))))
        else {
            panic!();
        };
        assert_eq!(g.group.as_deref(), Some("123-456@g.us"));
        assert_eq!(g.chat(), "123-456@g.us");
        assert_eq!(g.caller, "111222@lid");
    }

    #[test]
    fn answer_reject_terminate_map() {
        let creator = jid("111222@lid");
        let s = signal_from_call(&call(CallAction::Accept {
            call_id: "C1".into(),
            call_creator: creator.clone(),
            audio: Vec::new(),
        }));
        assert_eq!(
            s,
            Some(CallSignal::PeerAccepted {
                call_id: "C1".into()
            })
        );
        let s = signal_from_call(&call(CallAction::Reject {
            call_id: "C1".into(),
            call_creator: creator.clone(),
            reason: Some("busy".into()),
        }));
        assert_eq!(
            s,
            Some(CallSignal::PeerRejected {
                call_id: "C1".into(),
                busy: true
            })
        );
        let term = |reason: Option<&str>| {
            signal_from_call(&call(CallAction::Terminate {
                call_id: "C1".into(),
                call_creator: creator.clone(),
                reason: reason.map(Into::into),
                duration: None,
                audio_duration: None,
            }))
        };
        assert_eq!(
            term(None),
            Some(CallSignal::PeerEnded {
                call_id: "C1".into()
            })
        );
        assert_eq!(
            term(Some("timeout")),
            Some(CallSignal::PeerEnded {
                call_id: "C1".into()
            })
        );
        // Reported by CallEndedElsewhere instead.
        assert_eq!(term(Some("accepted_elsewhere")), None);
        assert_eq!(term(Some("rejected_elsewhere")), None);
        let s = signal_from_call(&call(CallAction::RelayLatency {
            call_id: "C1".into(),
            call_creator: creator,
        }));
        assert_eq!(s, None);
    }

    #[test]
    fn missed_and_elsewhere_events() {
        use whatsapp_rust::wacore::types::call::MissedReason;
        let ts = chrono::DateTime::from_timestamp(5, 0).unwrap();
        let m = MissedCall::new(
            jid("34600111222:2@s.whatsapp.net"),
            "C9".into(),
            ts,
            MissedReason::Offline,
        );
        assert_eq!(
            signal_from_missed(&m),
            CallSignal::Missed {
                call_id: "C9".into(),
                caller: "34600111222@s.whatsapp.net".into(),
                ts: 5
            }
        );
        let e = CallEndedElsewhere::new(jid("1@lid"), "C9".into(), ts, ElsewhereOutcome::Accepted);
        assert_eq!(
            signal_from_elsewhere(&e),
            CallSignal::Elsewhere {
                call_id: "C9".into(),
                accepted: true
            }
        );
    }

    #[test]
    fn history_call_logs() {
        use wa::message::call_log_message::CallOutcome as O;
        let c = wa::message::CallLogMessage {
            is_video: Some(false),
            call_outcome: Some(O::CONNECTED),
            duration_secs: Some(125),
            ..Default::default()
        };
        let l = log_from_message(&c);
        assert_eq!(l.outcome, CallOutcome::Answered);
        assert_eq!(l.seconds, 125);
        assert_eq!(l.preview(false), "Voice call (2 min)");
        let missed = wa::message::CallLogMessage {
            is_video: Some(true),
            call_outcome: Some(O::MISSED),
            ..Default::default()
        };
        assert_eq!(log_from_message(&missed).title(true), "Missed video call");
        use wa::web_message_info::StubType;
        assert_eq!(
            log_from_stub(StubType::CALL_MISSED_GROUP_VOICE)
                .unwrap()
                .title(true),
            "Missed group voice call"
        );
        assert!(log_from_stub(StubType::REVOKE).is_none());
    }

    #[test]
    fn talk_time_and_labels() {
        assert_eq!(talk_time(45), "45 sec");
        assert_eq!(talk_time(60), "1 min");
        assert_eq!(talk_time(3600), "1 hr");
        assert_eq!(talk_time(3900), "1 hr 5 min");
        let l = CallLog {
            video: false,
            group: false,
            outcome: CallOutcome::NoAnswer,
            seconds: 0,
        };
        assert_eq!(l.title(false), "Voice call");
        assert_eq!(l.detail(false), "No answer");
        assert!(!l.is_missed(false));
        let d = CallLog {
            outcome: CallOutcome::Declined,
            ..l.clone()
        };
        assert!(d.is_missed(true));
        assert_eq!(d.detail(true), "Click to call back");
        assert_eq!(
            CallLog {
                outcome: CallOutcome::Elsewhere,
                ..l
            }
            .detail(true),
            "Answered on another device"
        );
    }

    #[test]
    fn signals_debug_without_jids() {
        let s = CallSignal::Missed {
            call_id: "C".into(),
            caller: "34600111222@s.whatsapp.net".into(),
            ts: 0,
        };
        assert!(!format!("{s:?}").contains("346"));
    }
}
