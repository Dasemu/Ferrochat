//! The emoji drawer above the composer, like WhatsApp Desktop's: Emoji /
//! GIF / Stickers tabs. The emoji tab has category tabs, a search box and a
//! scrollable grid with section titles (recent emoji first), every cell
//! drawn with the bundled Twemoji font. The GIF and Stickers tabs are in
//! [`gifs`] and [`stickers`] (state in `app/picker.rs`).

use iced::widget::{
    Space, button, column, container, image, mouse_area, row, scrollable, stack, text, text_input,
};
use iced::{Alignment, Color, ContentFit, Length};

use super::anim_image::anim_image;
use super::emoji_font::emoji_font;
use super::theme::{self, INTER_MEDIUM};
use super::{El, icon};
use crate::app::emoji::{self, Category};
use crate::app::picker::{self, KeyState, PickerUi, Tab};
use crate::app::{App, Message, Ui};
use crate::media::sticker::ShelfItem;
use crate::ui::icons::Icon;

/// Height of the drawer (tabs, search and grid).
pub const HEIGHT: f32 = 340.0;

/// The drawer.
pub fn view(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let body = match app.picker.tab {
        Tab::Emoji => emoji_tab(app),
        Tab::Gifs => gifs(app),
        Tab::Stickers => stickers(app),
    };
    let tab = |label: &'static str, which: Tab| -> El<'_> {
        button(text(label).size(14).font(INTER_MEDIUM))
            .padding([5, 14])
            .style(theme::chip(t, app.picker.tab == which))
            .on_press(Message::Ui(Ui::Picker(PickerUi::Tab(which))))
            .into()
    };
    let tabs = container(
        row![
            tab("Emoji", Tab::Emoji),
            tab("GIF", Tab::Gifs),
            tab("Stickers", Tab::Stickers),
        ]
        .spacing(8),
    )
    .padding(iced::Padding {
        top: 8.0,
        right: 16.0,
        bottom: 0.0,
        left: 16.0,
    });
    container(
        container(column![tabs, body].height(HEIGHT))
            .width(Length::Fill)
            .style(theme::solid(p.bg_elevated)),
    )
    .height(Length::Fill)
    .align_y(Alignment::End)
    .into()
}

/// Width of the drawer's grid area.
fn pane_width(app: &App) -> f32 {
    let m = app.theme.metrics;
    app.window_size().width - m.rail_width - m.sidebar_width
}

/// The emoji tab.
fn emoji_tab(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let cols = emoji::columns(pane_width(app));
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

    column![
        container(tabs).padding([6, 12]),
        container(search).padding([0, 16]),
        Space::new().height(6),
        body,
    ]
    .into()
}

fn section_title<'a>(app: &'a App, title: &'a str) -> El<'a> {
    container(
        text(title)
            .size(13)
            .font(INTER_MEDIUM)
            .color(app.theme.palette.text_secondary),
    )
    .height(emoji::HEADER)
    .align_y(Alignment::Center)
    .into()
}

/// Tiles in rows of as many as fit the drawer.
fn grid<'a>(tiles: Vec<El<'a>>, cols: usize) -> El<'a> {
    let mut col = column![].spacing(picker::TILE_GAP);
    let mut it = tiles.into_iter().peekable();
    while it.peek().is_some() {
        let mut r = row![].spacing(picker::TILE_GAP);
        for tile in it.by_ref().take(cols.max(1)) {
            r = r.push(tile);
        }
        col = col.push(r);
    }
    col.into()
}

fn scroll_body<'a>(app: &'a App, content: El<'a>) -> El<'a> {
    scrollable(container(content).padding([4, 16]).width(Length::Fill))
        .id(emoji::SCROLL_ID)
        .direction(super::thin_scrollbar())
        .style(theme::scrollbar(&app.theme))
        .height(Length::Fill)
        .into()
}

fn note<'a>(app: &'a App, s: String) -> El<'a> {
    container(text(s).size(13).color(app.theme.palette.text_secondary))
        .padding([6, 0])
        .into()
}

fn sticker_tile<'a>(app: &'a App, item: &'a ShelfItem) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let side = picker::TILE;
    let pic: El<'a> = match app.picker.tiles.get(&item.key) {
        Some(h) => image(h.clone())
            .width(side - 8.0)
            .height(side - 8.0)
            .content_fit(ContentFit::Contain)
            .into(),
        None => icon(Icon::Smile, 28.0, p.text_muted),
    };
    let b = button(container(pic).center_x(side).center_y(side))
        .padding(0)
        .width(side)
        .height(side)
        .style(theme::icon_button(t, false))
        .on_press(Message::Ui(Ui::Picker(PickerUi::SendSticker(
            item.key.clone(),
        ))));
    // Right click: favourite / unfavourite.
    mouse_area(b)
        .on_right_press(Message::Ui(Ui::Picker(PickerUi::ToggleFavourite(
            item.key.clone(),
        ))))
        .into()
}

/// The Stickers tab: favourites, recent stickers, "Create".
fn stickers(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let pk = &app.picker;
    if let Some(d) = &pk.draft {
        let preview = image(d.handle.clone())
            .width(160)
            .height(160)
            .content_fit(ContentFit::Contain);
        let actions = row![
            button(text("Cancel").size(14))
                .padding([8, 18])
                .style(theme::link_button(t))
                .on_press(Message::Ui(Ui::Picker(PickerUi::DraftCancel))),
            button(text("Send sticker").size(14).font(INTER_MEDIUM))
                .padding([8, 18])
                .style(theme::primary_button(t))
                .on_press(Message::Ui(Ui::Picker(PickerUi::DraftSend))),
        ]
        .spacing(12);
        return container(
            column![preview, actions]
                .spacing(16)
                .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into();
    }
    let cols = ((pane_width(app) - 32.0 + picker::TILE_GAP) / (picker::TILE + picker::TILE_GAP))
        .floor()
        .max(1.0) as usize;
    let create = button(
        column![
            icon(Icon::Plus, 26.0, p.accent),
            text("Create").size(12).color(p.text_secondary)
        ]
        .spacing(4)
        .align_x(Alignment::Center),
    )
    .padding([18, 0])
    .width(picker::TILE)
    .height(picker::TILE)
    .style(theme::icon_button(t, false))
    .on_press(Message::Ui(Ui::Picker(PickerUi::CreateSticker)));
    let mut body = column![];
    if !pk.shown_favourites.is_empty() {
        body = body.push(section_title(app, "Favourites"));
        body = body.push(grid(
            pk.shown_favourites
                .iter()
                .map(|i| sticker_tile(app, i))
                .collect(),
            cols,
        ));
    }
    body = body.push(section_title(app, "Recent"));
    let mut tiles: Vec<El<'_>> = vec![create.into()];
    tiles.extend(pk.shown_stickers.iter().map(|i| sticker_tile(app, i)));
    body = body.push(grid(tiles, cols));
    if pk.shown_stickers.is_empty() {
        body = body.push(note(
            app,
            "Stickers you send or receive appear here. Right-click one to add it to your favourites."
                .into(),
        ));
    } else {
        body = body.push(note(
            app,
            "Right-click a sticker to add it to or remove it from your favourites.".into(),
        ));
    }
    scroll_body(app, body.into())
}

fn gif_tile<'a>(app: &'a App, item: &'a ShelfItem, w: f32) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let h = picker::GIF_TILE_H;
    let pic: El<'a> = match app.picker.tiles.get(&item.key) {
        Some(handle) => image(handle.clone())
            .width(w)
            .height(h)
            .content_fit(ContentFit::Contain)
            .into(),
        None => container(icon(Icon::Video, 24.0, p.text_muted))
            .center_x(w)
            .center_y(h)
            .into(),
    };
    let badge = container(
        container(text("GIF").size(10).font(INTER_MEDIUM).color(Color::WHITE))
            .padding([1, 5])
            .style(theme::rounded(Color::from_rgba(0.0, 0.0, 0.0, 0.55), 4.0)),
    )
    .padding(5)
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(Alignment::End);
    button(stack![pic, badge].width(w).height(h))
        .padding(0)
        .style(theme::icon_button(t, false))
        .on_press(Message::Ui(Ui::Picker(PickerUi::SendRecentGif(
            item.key.clone(),
        ))))
        .into()
}

fn hit_tile<'a>(app: &'a App, i: usize, hit: &'a crate::net::giphy::GifHit, w: f32) -> El<'a> {
    let t = &app.theme;
    let h = picker::GIF_TILE_H;
    let pic: El<'a> = match app.picker.hit_tiles.get(&hit.id) {
        Some(a) => anim_image(a.clone(), app.anim_epoch, app.media_playing(), w, h)
            .content_fit(ContentFit::Contain)
            .into(),
        None => container(Space::new())
            .width(w)
            .height(h)
            .style(theme::rounded(t.palette.bg_input, 6.0))
            .into(),
    };
    button(pic)
        .padding(0)
        .style(theme::icon_button(t, false))
        .on_press(Message::Ui(Ui::Picker(PickerUi::SendHit(i))))
        .into()
}

/// The GIF tab: GIPHY search (with the user's key), recent GIFs, "send a
/// GIF from a file".
fn gifs(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let pk = &app.picker;
    let cols = 4usize;
    let w =
        ((pane_width(app) - 32.0 - picker::TILE_GAP * (cols as f32 - 1.0)) / cols as f32).max(60.0);
    let top: El<'_> = match &pk.key {
        KeyState::Present(_) => container(
            row![
                icon(Icon::Search, 16.0, p.text_secondary),
                text_input("Search GIFs via GIPHY", &pk.query)
                    .id(emoji::SEARCH_ID)
                    .on_input(|q| Message::Ui(Ui::Picker(PickerUi::Query(q))))
                    .size(m.text)
                    .padding([6, 0])
                    .style(theme::input(t)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([2, 14])
        .width(Length::Fill)
        .style(theme::rounded(p.bg_input, 18.0))
        .into(),
        KeyState::Unknown | KeyState::Loading => Space::new().height(36).into(),
        KeyState::Missing | KeyState::Error(_) => container(
            row![
                text(
                    "GIF search uses GIPHY with your own free API key \
                     (developers.giphy.com). Add it in Settings."
                )
                .size(13)
                .color(p.text_secondary)
                .width(Length::Fill),
                button(text("Settings").size(13).font(INTER_MEDIUM))
                    .padding([6, 12])
                    .style(theme::link_button(t))
                    .on_press(Message::Ui(Ui::Picker(PickerUi::OpenSettings))),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([4, 14])
        .width(Length::Fill)
        .style(theme::rounded(p.bg_input, 8.0))
        .into(),
    };
    let mut body = column![];
    if let Some(b) = &pk.busy {
        body = body.push(note(app, b.clone()));
    }
    match &pk.results {
        Some(Ok(hits)) if !hits.is_empty() => {
            body = body.push(grid(
                hits.iter()
                    .enumerate()
                    .map(|(i, h)| hit_tile(app, i, h, w))
                    .collect(),
                cols,
            ));
            body = body.push(note(app, "Powered by GIPHY".into()));
        }
        Some(Ok(_)) => body = body.push(note(app, "No GIFs found".into())),
        Some(Err(e)) => body = body.push(note(app, e.clone())),
        None if pk.searching => body = body.push(note(app, "Searching…".into())),
        None => {
            body = body.push(section_title(app, "Recent"));
            if pk.shown_gifs.is_empty() {
                body = body.push(note(app, "GIFs you send or receive appear here.".into()));
            } else {
                body = body.push(grid(
                    pk.shown_gifs.iter().map(|i| gif_tile(app, i, w)).collect(),
                    cols,
                ));
            }
        }
    }
    let file = button(
        row![
            icon(Icon::Paperclip, 16.0, p.text_secondary),
            text("Send a GIF from a file")
                .size(13)
                .color(p.text_primary),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([6, 10])
    .style(theme::icon_button(t, false))
    .on_press(Message::Ui(Ui::Picker(PickerUi::SendGifFile)));
    column![
        container(top).padding([6, 16]),
        scroll_body(app, body.into()),
        container(file).padding([2, 12]),
    ]
    .into()
}
