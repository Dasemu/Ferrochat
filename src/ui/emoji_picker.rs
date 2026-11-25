//! The emoji drawer above the composer, like WhatsApp Desktop's: category
//! tabs, a search box and a scrollable grid with section titles (recent
//! emoji first). Every cell is drawn with the bundled Twemoji font.

use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Length};

use super::emoji_font::emoji_font;
use super::theme::{self, INTER_MEDIUM};
use super::{El, icon};
use crate::app::emoji::{self, Category};
use crate::app::{App, Message, Ui};
use crate::ui::icons::Icon;

/// Height of the drawer (tabs, search and grid).
pub const HEIGHT: f32 = 340.0;

/// The drawer.
pub fn view(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let pane = app.window_size().width - m.rail_width - m.sidebar_width;
    let cols = emoji::columns(pane);
    let searching = !app.emoji.query.trim().is_empty();

    let mut tabs = row![].spacing(2).align_y(Alignment::Center);
    for c in Category::ALL {
        if c == Category::Recent && app.emoji.shown_recent.is_empty() {
            continue;
        }
        tabs = tabs.push(
            button(text(c.tab_icon()).font(emoji_font()).size(20))
                .padding([4, 10])
                .style(theme::icon_button(t, false))
                .on_press(Message::Ui(Ui::EmojiCategory(c))),
        );
    }

    let search = container(
        row![
            icon(Icon::Search, 16.0, p.text_secondary),
            text_input("Search emoji", &app.emoji.query)
                .id(emoji::SEARCH_ID)
                .on_input(|q| Message::Ui(Ui::EmojiSearch(q)))
                .size(m.text)
                .padding([6, 0])
                .style(theme::input(t)),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([2, 14])
    .width(Length::Fill)
    .style(theme::rounded(p.bg_input, 18.0));

    let mut grid = column![].width(Length::Fill);
    let sections = app.emoji.sections();
    let empty = sections.iter().all(|s| s.emoji.is_empty());
    for s in sections {
        let title = if searching {
            "Search results"
        } else {
            s.category.title()
        };
        grid = grid.push(
            container(
                text(title)
                    .size(13)
                    .font(INTER_MEDIUM)
                    .color(p.text_secondary),
            )
            .height(emoji::HEADER)
            .align_y(Alignment::Center),
        );
        for chunk in s.emoji.chunks(cols) {
            let mut r = row![].height(emoji::CELL);
            for e in chunk {
                let Some(e) = emoji::intern(e) else {
                    continue;
                };
                r = r.push(
                    button(
                        container(text(e).font(emoji_font()).size(28))
                            .center_x(emoji::CELL)
                            .center_y(emoji::CELL),
                    )
                    .padding(0)
                    .width(emoji::CELL)
                    .height(emoji::CELL)
                    .style(theme::icon_button(t, false))
                    .on_press(Message::Ui(Ui::Emoji(e))),
                );
            }
            grid = grid.push(r);
        }
    }
    if searching && empty {
        grid = grid.push(
            container(text("No emoji found").size(m.text).color(p.text_secondary))
                .padding(20)
                .center_x(Length::Fill),
        );
    }
    let body = scrollable(grid.padding([0.0, emoji::GRID_PAD / 2.0]))
        .id(emoji::SCROLL_ID)
        .direction(super::thin_scrollbar())
        .style(theme::scrollbar(t))
        .height(Length::Fill);

    container(
        container(
            column![
                container(tabs).padding([6, 12]),
                container(search).padding([0, 16]),
                Space::new().height(6),
                body,
            ]
            .height(HEIGHT),
        )
        .width(Length::Fill)
        .style(theme::solid(p.bg_elevated)),
    )
    .height(Length::Fill)
    .align_y(Alignment::End)
    .into()
}
