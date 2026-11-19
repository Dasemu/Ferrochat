//! Headless driver for app-level tests: the real `App` and `Store` with the
//! fake protocol client. `Task`s are run to completion and their messages fed
//! back into `App::update`, like iced's runtime does.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use iced::Task;
use iced_runtime::Action;

use super::message::Message;
use super::{App, Ui};
use crate::config::Config;
use crate::mock;
use crate::net::Event;
use crate::net::fake::{self, FakeClient, FakeOptions};

/// `App` + fake client + the history the fake serves.
pub struct Harness {
    pub app: App,
    pub fake: Arc<FakeClient>,
}

impl Harness {
    /// Boots the app in mock mode and plays the fake's start-up events
    /// (session, connection, history, names, lists) into it.
    pub async fn new() -> Self {
        Self::with(FakeOptions {
            auto_receipts: false,
            ..FakeOptions::default()
        })
        .await
    }

    /// Like [`new`](Self::new) with fake options.
    pub async fn with(opts: FakeOptions) -> Self {
        let mut cfg = Config::defaults();
        cfg.mock = true;
        let (app, _boot) = App::boot(cfg);
        let (fake, stream) = fake::start(opts, mock::mock_data(1_700_000_000));
        let mut h = Self { app, fake };
        futures::pin_mut!(stream);
        let mut events = Vec::new();
        while let Ok(Some(ev)) =
            tokio::time::timeout(Duration::from_millis(50), stream.next()).await
        {
            events.push(ev);
        }
        for ev in events {
            h.net(ev).await;
        }
        h
    }

    /// Delivers a protocol event.
    pub async fn net(&mut self, ev: Event) {
        let t = self.app.update(Message::Net(ev));
        self.pump(t).await;
    }

    /// Applies a user action.
    pub async fn ui(&mut self, ui: Ui) {
        let t = self.app.update(Message::Ui(ui));
        self.pump(t).await;
    }

    /// Runs a task to completion, feeding its messages back.
    pub async fn pump(&mut self, task: Task<Message>) {
        let mut pending = vec![task];
        let mut rounds = 0;
        while let Some(task) = pending.pop() {
            rounds += 1;
            assert!(rounds < 500, "task loop did not settle");
            let Some(stream) = iced_runtime::task::into_stream(task) else {
                continue;
            };
            futures::pin_mut!(stream);
            let mut msgs = Vec::new();
            while let Ok(Some(action)) =
                tokio::time::timeout(Duration::from_secs(5), stream.next()).await
            {
                if let Action::Output(m) = action {
                    msgs.push(m);
                }
            }
            for m in msgs {
                pending.push(self.app.update(m));
            }
        }
    }
}
