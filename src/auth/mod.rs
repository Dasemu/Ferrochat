//! Login flow: QR-code pairing as a linked device.
//!
//! [`AuthState`] is a pure state machine driven by [`AuthEvent`]s that the
//! app derives from protocol events and user actions. It never performs I/O.

pub mod secrets;

/// Where the login flow is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// A stored session exists; connecting with it.
    Restoring,
    /// Not paired yet; waiting for the first QR code from the server.
    WaitingForQr,
    /// A QR code is on screen. `generation` increments on every refresh.
    ShowingQr {
        /// Payload to encode in the QR image.
        code: String,
        /// Seconds this code stays valid.
        valid_for_secs: u64,
        /// 1 for the first code, then +1 per refresh.
        generation: u32,
    },
    /// The server stopped rotating codes; the user must click to reload.
    QrExpired,
    /// The phone scanned the code; finishing the handshake.
    Pairing,
    /// Linked and connected (or connected before and now reconnecting).
    LoggedIn {
        /// Own JID, when known.
        jid: Option<String>,
    },
    /// Unlinked from the phone, or logged out locally.
    LoggedOut {
        /// Human-readable reason.
        reason: String,
    },
    /// Fatal problem before login (e.g. no Secret Service available).
    Failed {
        /// User-facing message (secret-free).
        message: String,
    },
}

/// Inputs to the auth state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEvent {
    /// Boot found an encrypted session store.
    SessionFound,
    /// Boot found no session store.
    NoSession,
    /// The server sent a (new) QR payload.
    Qr {
        /// QR payload.
        code: String,
        /// Validity in seconds.
        valid_for_secs: u64,
    },
    /// QR codes exhausted.
    QrExhausted,
    /// The user clicked "reload QR code".
    ReloadQr,
    /// Phone confirmed the pairing.
    PairSuccess {
        /// Own JID.
        jid: String,
    },
    /// Pairing failed.
    PairError(String),
    /// Connected to the server with a valid session.
    Connected {
        /// Own JID if the library knows it.
        jid: Option<String>,
    },
    /// Session revoked by the server / phone, or local logout finished.
    LoggedOut(String),
    /// Unrecoverable setup failure.
    Fatal(String),
}

impl Default for AuthState {
    fn default() -> Self {
        Self::WaitingForQr
    }
}

impl AuthState {
    /// Pure transition function.
    pub fn next(self, event: AuthEvent) -> AuthState {
        use AuthEvent as E;
        use AuthState as S;
        match (self, event) {
            (_, E::Fatal(message)) => S::Failed { message },
            (_, E::LoggedOut(reason)) => S::LoggedOut { reason },
            (_, E::SessionFound) => S::Restoring,
            (_, E::NoSession) => S::WaitingForQr,
            (
                S::ShowingQr { generation, .. },
                E::Qr {
                    code,
                    valid_for_secs,
                },
            ) => S::ShowingQr {
                code,
                valid_for_secs,
                generation: generation.saturating_add(1),
            },
            (S::LoggedIn { jid }, E::Qr { .. }) => S::LoggedIn { jid },
            (
                _,
                E::Qr {
                    code,
                    valid_for_secs,
                },
            ) => S::ShowingQr {
                code,
                valid_for_secs,
                generation: 1,
            },
            (S::ShowingQr { .. } | S::WaitingForQr, E::QrExhausted) => S::QrExpired,
            (s, E::QrExhausted) => s,
            (S::QrExpired | S::LoggedOut { .. } | S::Failed { .. }, E::ReloadQr) => S::WaitingForQr,
            (s, E::ReloadQr) => s,
            (_, E::PairSuccess { jid }) => S::LoggedIn { jid: Some(jid) },
            (_, E::PairError(message)) => S::Failed { message },
            (S::LoggedIn { jid: Some(old) }, E::Connected { jid: None }) => {
                S::LoggedIn { jid: Some(old) }
            }
            (_, E::Connected { jid }) => S::LoggedIn { jid },
        }
    }

    /// True once the user is past the QR screen.
    pub fn is_logged_in(&self) -> bool {
        matches!(self, Self::LoggedIn { .. } | Self::Restoring)
    }

    /// True while the login screen should be shown.
    pub fn shows_login(&self) -> bool {
        !self.is_logged_in()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qr(c: &str) -> AuthEvent {
        AuthEvent::Qr {
            code: c.into(),
            valid_for_secs: 20,
        }
    }

    #[test]
    fn fresh_pairing_flow() {
        let s = AuthState::default().next(AuthEvent::NoSession);
        assert_eq!(s, AuthState::WaitingForQr);
        let s = s.next(qr("a"));
        assert!(matches!(s, AuthState::ShowingQr { generation: 1, .. }));
        let s = s.next(qr("b"));
        match &s {
            AuthState::ShowingQr {
                code, generation, ..
            } => {
                assert_eq!(code, "b");
                assert_eq!(*generation, 2);
            }
            other => panic!("{other:?}"),
        }
        let s = s.next(AuthEvent::PairSuccess {
            jid: "1@s.whatsapp.net".into(),
        });
        assert!(s.is_logged_in());
        let s = s.next(AuthEvent::Connected { jid: None });
        assert_eq!(
            s,
            AuthState::LoggedIn {
                jid: Some("1@s.whatsapp.net".into())
            }
        );
    }

    #[test]
    fn qr_exhaustion_and_reload() {
        let s = AuthState::WaitingForQr
            .next(qr("a"))
            .next(AuthEvent::QrExhausted);
        assert_eq!(s, AuthState::QrExpired);
        let s = s.next(AuthEvent::ReloadQr);
        assert_eq!(s, AuthState::WaitingForQr);
        // Reload is ignored while a code is still valid.
        let showing = AuthState::WaitingForQr.next(qr("x"));
        assert!(matches!(
            showing.clone().next(AuthEvent::ReloadQr),
            AuthState::ShowingQr { .. }
        ));
    }

    #[test]
    fn restore_then_connect() {
        let s = AuthState::default().next(AuthEvent::SessionFound);
        assert!(s.is_logged_in());
        let s = s.next(AuthEvent::Connected {
            jid: Some("me".into()),
        });
        assert_eq!(
            s,
            AuthState::LoggedIn {
                jid: Some("me".into())
            }
        );
        // A stray QR while logged in does not kick the user out.
        assert!(s.next(qr("z")).is_logged_in());
    }

    #[test]
    fn revoked_session_returns_to_login() {
        let s = AuthState::LoggedIn { jid: None }.next(AuthEvent::LoggedOut("unlinked".into()));
        assert!(s.shows_login());
        assert_eq!(s.next(AuthEvent::ReloadQr), AuthState::WaitingForQr);
    }

    #[test]
    fn fatal_is_sticky_until_reload() {
        let s = AuthState::WaitingForQr.next(AuthEvent::Fatal("no keyring".into()));
        assert!(matches!(s, AuthState::Failed { .. }));
        assert_eq!(s.next(AuthEvent::ReloadQr), AuthState::WaitingForQr);
    }
}
