//! Calls on the app side: the call state machine (pure, unit tested) and
//! the audio devices (`audio`).
//!
//! The state machine takes protocol signals ([`CallSignal`]) and user
//! actions and answers with [`Effect`]s the app carries out (send a decline,
//! start the ringtone, write a call bubble, ...). Time comes in as Unix
//! seconds so the tests control it.

pub mod audio;

use std::collections::VecDeque;

use crate::net::{CallLog, CallOutcome, CallSignal, ChatId, IncomingCallInfo, is_group_jid};

/// An outgoing call that nobody answers is cancelled after this long, like
/// the official client.
pub const RING_TIMEOUT_SECS: i64 = 60;
/// Incoming offers remembered (so a late "missed" knows chat and media).
const MAX_SEEN: usize = 16;
/// Call ids already written to a chat (each call is logged once).
const MAX_LOGGED: usize = 64;

/// Where the current call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Ringing here; waiting for Accept / Decline.
    Incoming,
    /// We are placing the call (offer not sent yet).
    Calling,
    /// Offer sent; the peer's phone rings.
    Ringing,
    /// Answered; media is being set up.
    Connecting,
    /// Talking since this Unix second.
    Active(i64),
}

/// The call shown in the call panel (at most one at a time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Call id (unknown while an outgoing call is being placed).
    pub call_id: Option<String>,
    /// The chat the call belongs to.
    pub chat: ChatId,
    /// The other person (the caller in a group call).
    pub peer: String,
    /// Incoming (true) or placed by us.
    pub incoming: bool,
    /// Video was offered.
    pub video: bool,
    /// Group call (answer on the phone).
    pub group: bool,
    /// Phase.
    pub phase: Phase,
    /// Our microphone is muted.
    pub muted: bool,
    /// When the current phase started (Unix seconds).
    pub since: i64,
}

impl Call {
    /// The status line of the call panel.
    pub fn status(&self, now: i64) -> String {
        match self.phase {
            Phase::Incoming if self.video => "Incoming video call".into(),
            Phase::Incoming => "Incoming voice call".into(),
            Phase::Calling => "Calling…".into(),
            Phase::Ringing => "Ringing…".into(),
            Phase::Connecting => "Connecting…".into(),
            Phase::Active(t) => {
                let s = (now - t).max(0) as u32;
                if s >= 3600 {
                    format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
                } else {
                    format!("{:02}:{:02}", s / 60, s % 60)
                }
            }
        }
    }

    /// Can this device answer it? Group calls and video media are not
    /// implemented: those are answered on the phone (a video call can still
    /// be answered as voice only).
    pub fn can_answer(&self) -> bool {
        !self.group
    }
}

/// Something the app must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start the ringtone, notify, and bring the window up.
    Ring,
    /// Stop the ringtone.
    StopRing,
    /// Send the decline for this call.
    Reject(String),
    /// Answer this call (open the audio devices first).
    Accept(String),
    /// Place a call to this chat (open the audio devices first).
    Start(ChatId),
    /// Hang up / cancel this call.
    End(String),
    /// Close the audio devices.
    CloseAudio,
    /// Write a call bubble into a chat.
    Log(LogEntry),
    /// Tell the user something.
    Toast(String),
}

/// A call bubble to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Message id (`call-<call id>`, so redelivery is idempotent).
    pub id: String,
    /// Chat.
    pub chat: ChatId,
    /// Who called (incoming) — empty for ours.
    pub sender: String,
    /// Incoming call.
    pub incoming: bool,
    /// Unix seconds.
    pub ts: i64,
    /// What happened.
    pub log: CallLog,
}

/// All call state of the app.
#[derive(Debug, Default)]
pub struct Calls {
    /// The call in the panel.
    pub current: Option<Call>,
    /// Offers seen (also ones that rang while we were busy).
    seen: VecDeque<IncomingCallInfo>,
    /// Calls already logged.
    logged: VecDeque<String>,
}

impl Calls {
    /// Is a call in progress (ringing, placing or talking)?
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    fn is_current(&self, call_id: &str) -> bool {
        self.current
            .as_ref()
            .and_then(|c| c.call_id.as_deref())
            .is_some_and(|id| id == call_id)
    }

    fn remember(&mut self, info: &IncomingCallInfo) {
        self.seen.retain(|i| i.call_id != info.call_id);
        if self.seen.len() >= MAX_SEEN {
            self.seen.pop_front();
        }
        self.seen.push_back(info.clone());
    }

    fn log(&mut self, call_id: &str, entry: LogEntry, out: &mut Vec<Effect>) {
        if self.logged.iter().any(|id| id == call_id) {
            return;
        }
        if self.logged.len() >= MAX_LOGGED {
            self.logged.pop_front();
        }
        self.logged.push_back(call_id.to_string());
        out.push(Effect::Log(entry));
    }

    /// Ends the current call and logs it with `outcome`.
    fn finish(&mut self, outcome: CallOutcome, now: i64, out: &mut Vec<Effect>) {
        let Some(call) = self.current.take() else {
            return;
        };
        if call.phase == Phase::Incoming {
            out.push(Effect::StopRing);
        } else {
            out.push(Effect::CloseAudio);
        }
        let Some(id) = call.call_id.clone() else {
            return;
        };
        let seconds = match call.phase {
            Phase::Active(t) => (now - t).clamp(0, i64::from(u32::MAX)) as u32,
            _ => 0,
        };
        let entry = LogEntry {
            id: format!("call-{id}"),
            chat: call.chat.clone(),
            sender: if call.incoming {
                call.peer.clone()
            } else {
                String::new()
            },
            incoming: call.incoming,
            ts: now,
            log: CallLog {
                video: call.video,
                group: call.group,
                outcome,
                seconds,
            },
        };
        self.log(&id, entry, out);
    }

    /// Logs a call that is not the current one (it rang while we were busy,
    /// or the offer came from the offline queue).
    fn log_other(
        &mut self,
        call_id: &str,
        caller: &str,
        ts: i64,
        outcome: CallOutcome,
        out: &mut Vec<Effect>,
    ) {
        let info = self.seen.iter().find(|i| i.call_id == call_id).cloned();
        let (chat, sender, video, group) = match &info {
            Some(i) => (
                i.chat().to_string(),
                i.caller.clone(),
                i.video,
                i.group.is_some(),
            ),
            None if !caller.is_empty() => (caller.to_string(), caller.to_string(), false, false),
            None => return,
        };
        let entry = LogEntry {
            id: format!("call-{call_id}"),
            chat,
            sender,
            incoming: true,
            ts,
            log: CallLog {
                video,
                group,
                outcome,
                seconds: 0,
            },
        };
        self.log(call_id, entry, out);
    }

    /// Applies a protocol signal.
    pub fn on_signal(&mut self, signal: CallSignal, now: i64) -> Vec<Effect> {
        let mut out = Vec::new();
        match signal {
            CallSignal::Ringing(info) => {
                self.remember(&info);
                if self.busy() || self.logged.iter().any(|id| *id == info.call_id) {
                    // Busy: the caller's other devices / the phone still ring;
                    // it is logged as missed when it ends.
                    return out;
                }
                self.current = Some(Call {
                    call_id: Some(info.call_id.clone()),
                    chat: info.chat().to_string(),
                    peer: info.caller.clone(),
                    incoming: true,
                    video: info.video,
                    group: info.group.is_some() || is_group_jid(info.chat()),
                    phase: Phase::Incoming,
                    muted: false,
                    since: now,
                });
                out.push(Effect::Ring);
            }
            CallSignal::Missed {
                call_id,
                caller,
                ts,
            } => {
                if self.is_current(&call_id) {
                    self.finish(CallOutcome::Missed, now, &mut out);
                } else {
                    self.log_other(&call_id, &caller, ts, CallOutcome::Missed, &mut out);
                }
            }
            CallSignal::Elsewhere { call_id, accepted } => {
                let outcome = if accepted {
                    CallOutcome::Elsewhere
                } else {
                    CallOutcome::Declined
                };
                if self.is_current(&call_id) {
                    self.finish(outcome, now, &mut out);
                } else {
                    self.log_other(&call_id, "", now, outcome, &mut out);
                }
            }
            CallSignal::PeerAccepted { call_id } => {
                if let Some(c) = self.current.as_mut().filter(|c| !c.incoming)
                    && c.call_id.as_deref() == Some(call_id.as_str())
                    && matches!(c.phase, Phase::Calling | Phase::Ringing)
                {
                    c.phase = Phase::Connecting;
                    c.since = now;
                }
            }
            CallSignal::Connected { call_id } => {
                if let Some(c) = self.current.as_mut()
                    && c.call_id.as_deref() == Some(call_id.as_str())
                    && !matches!(c.phase, Phase::Active(_) | Phase::Incoming)
                {
                    c.phase = Phase::Active(now);
                    c.since = now;
                }
            }
            CallSignal::PeerRejected { call_id, busy } => {
                // `busy` speaks for one device only; the others keep ringing.
                if !busy
                    && self.is_current(&call_id)
                    && self.current.as_ref().is_some_and(|c| !c.incoming)
                {
                    self.finish(CallOutcome::Declined, now, &mut out);
                }
            }
            CallSignal::PeerEnded { call_id } => {
                if let Some(c) = self.current.as_ref().filter(|_| self.is_current(&call_id)) {
                    let outcome = match c.phase {
                        Phase::Active(_) => CallOutcome::Answered,
                        Phase::Incoming => CallOutcome::Missed,
                        Phase::Connecting if c.incoming => CallOutcome::Failed,
                        _ => CallOutcome::NoAnswer,
                    };
                    self.finish(outcome, now, &mut out);
                } else if self.seen.iter().any(|i| i.call_id == call_id) {
                    self.log_other(&call_id, "", now, CallOutcome::Missed, &mut out);
                }
            }
            CallSignal::MediaEnded { call_id, error } => {
                if let Some(c) = self.current.as_ref().filter(|_| self.is_current(&call_id)) {
                    let outcome = match c.phase {
                        Phase::Active(_) => CallOutcome::Answered,
                        Phase::Ringing | Phase::Calling => CallOutcome::NoAnswer,
                        _ => CallOutcome::Failed,
                    };
                    if let Some(e) = error {
                        out.push(Effect::Toast(e));
                    }
                    self.finish(outcome, now, &mut out);
                }
            }
        }
        out
    }

    /// The user declined the ringing call.
    pub fn decline(&mut self, now: i64) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(id) = self
            .current
            .as_ref()
            .filter(|c| c.phase == Phase::Incoming)
            .and_then(|c| c.call_id.clone())
        else {
            return out;
        };
        out.push(Effect::Reject(id));
        self.finish(CallOutcome::Declined, now, &mut out);
        out
    }

    /// The user answered the ringing call.
    pub fn accept(&mut self, now: i64) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(c) = self
            .current
            .as_mut()
            .filter(|c| c.phase == Phase::Incoming && c.can_answer())
        else {
            return out;
        };
        let Some(id) = c.call_id.clone() else {
            return out;
        };
        c.phase = Phase::Connecting;
        c.since = now;
        out.push(Effect::StopRing);
        out.push(Effect::Accept(id));
        out
    }

    /// The user starts a voice call from a chat.
    pub fn start(&mut self, chat: &str, now: i64) -> Vec<Effect> {
        if self.busy() {
            return vec![Effect::Toast("You're already in a call.".into())];
        }
        if is_group_jid(chat) {
            return vec![Effect::Toast(
                "Group calls are not supported yet. Start the call on your phone.".into(),
            )];
        }
        self.current = Some(Call {
            call_id: None,
            chat: chat.to_string(),
            peer: chat.to_string(),
            incoming: false,
            video: false,
            group: false,
            phase: Phase::Calling,
            muted: false,
            since: now,
        });
        vec![Effect::Start(chat.to_string())]
    }

    /// The call was placed (`Ok(call id)`) or could not be.
    pub fn started(&mut self, chat: &str, result: Result<String, String>, now: i64) -> Vec<Effect> {
        let mut out = Vec::new();
        let placing = self
            .current
            .as_ref()
            .is_some_and(|c| !c.incoming && c.phase == Phase::Calling && c.chat == chat);
        match result {
            Ok(id) if placing => {
                if let Some(c) = self.current.as_mut() {
                    c.call_id = Some(id);
                    c.phase = Phase::Ringing;
                    c.since = now;
                }
            }
            // Hung up while it was being placed: cancel it at once.
            Ok(id) => out.push(Effect::End(id)),
            Err(e) if placing => {
                self.current = None;
                out.push(Effect::CloseAudio);
                out.push(Effect::Toast(format!("Couldn't start the call: {e}")));
            }
            Err(_) => {}
        }
        out
    }

    /// The user hung up (or cancelled an outgoing call).
    pub fn hang_up(&mut self, now: i64) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(c) = self.current.as_ref() else {
            return out;
        };
        if c.phase == Phase::Incoming {
            return self.decline(now);
        }
        let outcome = match c.phase {
            Phase::Active(_) => CallOutcome::Answered,
            _ if c.incoming => CallOutcome::Failed,
            _ => CallOutcome::NoAnswer,
        };
        if let Some(id) = c.call_id.clone() {
            out.push(Effect::End(id));
        }
        self.finish(outcome, now, &mut out);
        out
    }

    /// Toggles our microphone; returns the call id and the new state.
    pub fn toggle_mute(&mut self) -> Option<(String, bool)> {
        let c = self.current.as_mut()?;
        c.muted = !c.muted;
        Some((c.call_id.clone()?, c.muted))
    }

    /// Once a second while a call is up: gives up on unanswered calls.
    pub fn tick(&mut self, now: i64) -> Vec<Effect> {
        match &self.current {
            Some(c)
                if !c.incoming
                    && c.phase == Phase::Ringing
                    && now - c.since >= RING_TIMEOUT_SECS =>
            {
                self.hang_up(now)
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOB: &str = "34600111222@s.whatsapp.net";

    fn ring(id: &str, video: bool) -> CallSignal {
        CallSignal::Ringing(IncomingCallInfo {
            call_id: id.into(),
            caller: BOB.into(),
            group: None,
            video,
            ts: 100,
        })
    }

    fn logs(effects: &[Effect]) -> Vec<LogEntry> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Log(l) => Some(l.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn incoming_call_rings_and_decline_rejects_and_logs() {
        let mut c = Calls::default();
        let fx = c.on_signal(ring("A", false), 100);
        assert_eq!(fx, vec![Effect::Ring]);
        assert_eq!(
            c.current.as_ref().unwrap().status(100),
            "Incoming voice call"
        );
        let fx = c.decline(105);
        assert!(fx.contains(&Effect::Reject("A".into())));
        assert!(fx.contains(&Effect::StopRing));
        let l = logs(&fx);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id, "call-A");
        assert_eq!(l[0].chat, BOB);
        assert!(l[0].incoming);
        assert_eq!(l[0].log.outcome, CallOutcome::Declined);
        assert!(c.current.is_none());
        // The caller's terminate after our reject logs nothing more.
        assert!(
            logs(&c.on_signal(
                CallSignal::PeerEnded {
                    call_id: "A".into()
                },
                106
            ))
            .is_empty()
        );
        assert!(
            logs(&c.on_signal(
                CallSignal::Missed {
                    call_id: "A".into(),
                    caller: BOB.into(),
                    ts: 106
                },
                106
            ))
            .is_empty()
        );
    }

    #[test]
    fn caller_giving_up_is_a_missed_call_logged_once() {
        let mut c = Calls::default();
        c.on_signal(ring("A", true), 100);
        let fx = c.on_signal(
            CallSignal::PeerEnded {
                call_id: "A".into(),
            },
            130,
        );
        assert!(fx.contains(&Effect::StopRing));
        let l = logs(&fx);
        assert_eq!(l[0].log.outcome, CallOutcome::Missed);
        assert!(l[0].log.video);
        assert_eq!(l[0].log.title(true), "Missed video call");
        let fx = c.on_signal(
            CallSignal::Missed {
                call_id: "A".into(),
                caller: BOB.into(),
                ts: 130,
            },
            130,
        );
        assert!(logs(&fx).is_empty());
    }

    #[test]
    fn offline_missed_call_is_logged_without_ringing() {
        let mut c = Calls::default();
        let fx = c.on_signal(
            CallSignal::Missed {
                call_id: "Z".into(),
                caller: BOB.into(),
                ts: 50,
            },
            100,
        );
        assert!(!fx.contains(&Effect::Ring));
        let l = logs(&fx);
        assert_eq!(l[0].chat, BOB);
        assert_eq!(l[0].ts, 50);
        assert!(c.current.is_none());
    }

    #[test]
    fn answered_elsewhere_stops_ringing() {
        let mut c = Calls::default();
        c.on_signal(ring("A", false), 100);
        let fx = c.on_signal(
            CallSignal::Elsewhere {
                call_id: "A".into(),
                accepted: true,
            },
            104,
        );
        assert!(fx.contains(&Effect::StopRing));
        assert_eq!(logs(&fx)[0].log.outcome, CallOutcome::Elsewhere);
        assert!(c.current.is_none());
    }

    #[test]
    fn accepted_call_runs_and_logs_its_duration() {
        let mut c = Calls::default();
        c.on_signal(ring("A", false), 100);
        let fx = c.accept(101);
        assert_eq!(fx, vec![Effect::StopRing, Effect::Accept("A".into())]);
        c.on_signal(
            CallSignal::Connected {
                call_id: "A".into(),
            },
            102,
        );
        assert_eq!(c.current.as_ref().unwrap().status(167), "01:05");
        let fx = c.on_signal(
            CallSignal::PeerEnded {
                call_id: "A".into(),
            },
            227,
        );
        assert!(fx.contains(&Effect::CloseAudio));
        let l = logs(&fx);
        assert_eq!(l[0].log.outcome, CallOutcome::Answered);
        assert_eq!(l[0].log.seconds, 125);
        assert_eq!(l[0].log.detail(true), "2 min");
    }

    #[test]
    fn group_calls_ring_but_cannot_be_answered_here() {
        let mut c = Calls::default();
        c.on_signal(
            CallSignal::Ringing(IncomingCallInfo {
                call_id: "G".into(),
                caller: BOB.into(),
                group: Some("123@g.us".into()),
                video: false,
                ts: 1,
            }),
            1,
        );
        assert!(!c.current.as_ref().unwrap().can_answer());
        assert!(c.accept(2).is_empty());
        let fx = c.decline(3);
        assert_eq!(logs(&fx)[0].chat, "123@g.us");
    }

    #[test]
    fn second_call_while_busy_does_not_ring_and_is_missed() {
        let mut c = Calls::default();
        c.on_signal(ring("A", false), 1);
        assert!(c.on_signal(ring("B", false), 2).is_empty());
        let fx = c.on_signal(
            CallSignal::PeerEnded {
                call_id: "B".into(),
            },
            9,
        );
        assert_eq!(logs(&fx)[0].id, "call-B");
        assert_eq!(c.current.as_ref().unwrap().call_id.as_deref(), Some("A"));
    }

    #[test]
    fn outgoing_call_flow() {
        let mut c = Calls::default();
        assert_eq!(c.start(BOB, 0), vec![Effect::Start(BOB.into())]);
        assert_eq!(c.current.as_ref().unwrap().status(0), "Calling…");
        c.started(BOB, Ok("X".into()), 1);
        assert_eq!(c.current.as_ref().unwrap().phase, Phase::Ringing);
        c.on_signal(
            CallSignal::PeerAccepted {
                call_id: "X".into(),
            },
            5,
        );
        c.on_signal(
            CallSignal::Connected {
                call_id: "X".into(),
            },
            6,
        );
        let fx = c.hang_up(66);
        assert!(fx.contains(&Effect::End("X".into())));
        let l = logs(&fx);
        assert!(!l[0].incoming);
        assert_eq!(l[0].sender, "");
        assert_eq!(l[0].log.seconds, 60);
        // The engine's end after our hang-up changes nothing.
        assert!(
            c.on_signal(
                CallSignal::MediaEnded {
                    call_id: "X".into(),
                    error: None
                },
                67
            )
            .is_empty()
        );
    }

    #[test]
    fn unanswered_outgoing_call_times_out() {
        let mut c = Calls::default();
        c.start(BOB, 0);
        c.started(BOB, Ok("X".into()), 0);
        assert!(c.tick(30).is_empty());
        let fx = c.tick(RING_TIMEOUT_SECS);
        assert!(fx.contains(&Effect::End("X".into())));
        assert_eq!(logs(&fx)[0].log.outcome, CallOutcome::NoAnswer);
    }

    #[test]
    fn peer_decline_ends_but_busy_does_not() {
        let mut c = Calls::default();
        c.start(BOB, 0);
        c.started(BOB, Ok("X".into()), 0);
        assert!(
            c.on_signal(
                CallSignal::PeerRejected {
                    call_id: "X".into(),
                    busy: true
                },
                1
            )
            .is_empty()
        );
        let fx = c.on_signal(
            CallSignal::PeerRejected {
                call_id: "X".into(),
                busy: false,
            },
            2,
        );
        assert_eq!(logs(&fx)[0].log.outcome, CallOutcome::Declined);
        assert_eq!(logs(&fx)[0].log.detail(false), "Declined");
    }

    #[test]
    fn failed_start_and_group_and_busy_starts_toast() {
        let mut c = Calls::default();
        c.start(BOB, 0);
        let fx = c.started(BOB, Err("offline".into()), 1);
        assert!(matches!(fx.last(), Some(Effect::Toast(_))));
        assert!(c.current.is_none());
        assert!(matches!(c.start("1@g.us", 0)[0], Effect::Toast(_)));
        c.on_signal(ring("A", false), 0);
        assert!(matches!(c.start(BOB, 0)[0], Effect::Toast(_)));
    }

    #[test]
    fn hang_up_while_placing_cancels_the_late_call() {
        let mut c = Calls::default();
        c.start(BOB, 0);
        c.hang_up(1);
        assert!(c.current.is_none());
        assert_eq!(
            c.started(BOB, Ok("X".into()), 2),
            vec![Effect::End("X".into())]
        );
    }

    #[test]
    fn mute_toggles() {
        let mut c = Calls::default();
        assert!(c.toggle_mute().is_none());
        c.start(BOB, 0);
        c.started(BOB, Ok("X".into()), 0);
        assert_eq!(c.toggle_mute(), Some(("X".into(), true)));
        assert_eq!(c.toggle_mute(), Some(("X".into(), false)));
    }
}
