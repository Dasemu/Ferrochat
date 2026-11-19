//! Window / tray state machine (pure, unit-tested).
//!
//! The window is a real iced window that is *closed* (not just hidden) when
//! the app goes to the tray, so the renderer and its surfaces are freed. This
//! type decides what each event means; `App` turns the returned [`Effect`]
//! into iced tasks.
//!
//! Invariant: the process never stays running with no window and no tray.

/// What the app must do after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing.
    None,
    /// Open the main window (restoring the last size and position).
    OpenWindow,
    /// Close the main window; the app keeps running in the tray.
    CloseWindow,
    /// Bring the existing window to the front.
    Focus,
    /// Leave the app.
    Exit,
}

/// Window visibility plus whether a tray exists to come back from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowMode {
    /// The main window exists (is, or is about to be, shown).
    pub open: bool,
    /// A StatusNotifierWatcher accepted our icon.
    pub tray_up: bool,
    /// Setting: closing the window keeps the app running.
    pub close_to_tray: bool,
}

impl WindowMode {
    /// Initial state. With `minimized` no window is opened at start.
    pub fn start(minimized: bool, close_to_tray: bool) -> Self {
        Self {
            open: !minimized,
            tray_up: false,
            close_to_tray,
        }
    }

    /// Closing hides to the tray only if the setting is on and a tray exists.
    pub fn hides_on_close(&self) -> bool {
        self.close_to_tray && self.tray_up
    }

    /// The user closed the window (X button, Alt+F4).
    pub fn close_requested(&mut self) -> Effect {
        if !self.open {
            return Effect::None;
        }
        if self.hides_on_close() {
            self.open = false;
            Effect::CloseWindow
        } else {
            Effect::Exit
        }
    }

    /// Show the window (tray menu, second launch, notification click).
    pub fn show(&mut self) -> Effect {
        if self.open {
            Effect::Focus
        } else {
            self.open = true;
            Effect::OpenWindow
        }
    }

    /// Left click on the tray icon / Show-Hide entry.
    pub fn toggle(&mut self) -> Effect {
        if !self.open {
            self.show()
        } else if self.tray_up {
            self.open = false;
            Effect::CloseWindow
        } else {
            Effect::Focus
        }
    }

    /// The window disappeared without us asking (compositor closed it).
    pub fn window_closed(&mut self) -> Effect {
        if !self.open {
            return Effect::None; // we closed it ourselves
        }
        self.open = false;
        if self.tray_up {
            Effect::None
        } else {
            Effect::Exit
        }
    }

    /// The tray registered or vanished. Losing it while hidden reopens the
    /// window so the app is never unreachable.
    pub fn tray_changed(&mut self, up: bool) -> Effect {
        self.tray_up = up;
        if !up && !self.open {
            self.open = true;
            Effect::OpenWindow
        } else {
            Effect::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tray() -> WindowMode {
        let mut m = WindowMode::start(false, true);
        m.tray_changed(true);
        m
    }

    #[test]
    fn visible_hidden_shown() {
        let mut m = with_tray();
        assert!(m.open);
        assert_eq!(m.close_requested(), Effect::CloseWindow);
        assert!(!m.open);
        assert_eq!(m.show(), Effect::OpenWindow);
        assert!(m.open);
        assert_eq!(m.show(), Effect::Focus);
    }

    #[test]
    fn close_without_tray_quits() {
        let mut m = WindowMode::start(false, true);
        assert_eq!(m.close_requested(), Effect::Exit);
    }

    #[test]
    fn close_with_setting_off_quits_even_with_tray() {
        let mut m = WindowMode::start(false, false);
        m.tray_changed(true);
        assert_eq!(m.close_requested(), Effect::Exit);
        // ... but an explicit Hide from the tray still works.
        let mut m = WindowMode::start(false, false);
        m.tray_changed(true);
        assert_eq!(m.toggle(), Effect::CloseWindow);
    }

    #[test]
    fn toggle_flips_between_shown_and_hidden() {
        let mut m = with_tray();
        assert_eq!(m.toggle(), Effect::CloseWindow);
        assert_eq!(m.toggle(), Effect::OpenWindow);
        assert_eq!(m.toggle(), Effect::CloseWindow);
    }

    #[test]
    fn toggle_without_tray_never_hides() {
        let mut m = WindowMode::start(false, true);
        assert_eq!(m.toggle(), Effect::Focus);
        assert!(m.open);
    }

    #[test]
    fn minimized_start_has_no_window_until_asked() {
        let mut m = WindowMode::start(true, true);
        assert!(!m.open);
        assert_eq!(m.tray_changed(true), Effect::None);
        assert!(!m.open);
        assert_eq!(m.show(), Effect::OpenWindow);
    }

    #[test]
    fn minimized_start_without_a_tray_opens_the_window() {
        let mut m = WindowMode::start(true, true);
        assert_eq!(m.tray_changed(false), Effect::OpenWindow);
        assert!(m.open);
    }

    #[test]
    fn losing_the_tray_while_hidden_reopens() {
        let mut m = with_tray();
        m.close_requested();
        assert_eq!(m.tray_changed(false), Effect::OpenWindow);
        assert!(m.open);
        // And a shown window just stays.
        let mut m = with_tray();
        assert_eq!(m.tray_changed(false), Effect::None);
    }

    #[test]
    fn our_own_close_event_is_ignored_but_a_foreign_one_exits_without_tray() {
        let mut m = with_tray();
        m.close_requested();
        assert_eq!(m.window_closed(), Effect::None);
        let mut m = WindowMode::start(false, true);
        assert_eq!(m.window_closed(), Effect::Exit);
        let mut m = with_tray();
        assert_eq!(m.window_closed(), Effect::None);
        assert!(!m.open);
    }

    #[test]
    fn repeated_close_requests_while_hidden_do_nothing() {
        let mut m = with_tray();
        m.close_requested();
        assert_eq!(m.close_requested(), Effect::None);
    }
}
