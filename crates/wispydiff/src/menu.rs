//! A small pick-one menu (the web app's `<select>`s): it takes focus when it opens, ↑/↓ move,
//! Enter or Space picks, Escape or a click outside closes and gives focus back.
//!
//! The owner keeps the `Entity<Menu>` while it's open, subscribes to `MenuEvent`, and draws it
//! where it belongs (usually `deferred(anchored().child(menu))` under the trigger).

use gpui_kit::prelude::*;
use std::time::{Duration, Instant};

use gpui_kit::{actions, div, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, Render, SharedString, Subscription, Window};

use crate::ui::{palette, u};

actions!(wispy_menu, [MenuUp, MenuDown, MenuPick, MenuClose]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", MenuUp, Some("Menu")),
        KeyBinding::new("down", MenuDown, Some("Menu")),
        KeyBinding::new("enter", MenuPick, Some("Menu")),
        KeyBinding::new("space", MenuPick, Some("Menu")),
        KeyBinding::new("escape", MenuClose, Some("Menu")),
        KeyBinding::new("tab", MenuClose, Some("Menu")),
    ]);
}

/// The one open menu of a view (keyed by trigger id).
#[derive(Default)]
pub struct MenuSlot {
    open: Option<(SharedString, Entity<Menu>, Subscription)>,
    /// The last menu closed by a click outside: a click on its own trigger closes it, not reopens.
    closed: Option<(SharedString, Instant)>,
}

impl MenuSlot {
    pub fn get(&self, id: &str) -> Option<Entity<Menu>> {
        self.open.as_ref().filter(|(open, ..)| open == id).map(|(_, menu, _)| menu.clone())
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Opens `id`'s menu, or closes it if it's open. `pick` gets the picked option's index.
    #[allow(clippy::too_many_arguments)]
    pub fn toggle<T: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        options: Vec<SharedString>,
        selected: Option<usize>,
        slot: fn(&mut T) -> &mut MenuSlot,
        pick: impl Fn(&mut T, usize, &mut Window, &mut Context<T>) + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let id = id.into();
        if self.get(&id).is_some() {
            self.close(window, cx);
            return;
        }
        if self.closed.as_ref().is_some_and(|(closed, at)| *closed == id && at.elapsed() < Duration::from_millis(300)) {
            self.closed = None;
            return;
        }
        self.close(window, cx);
        let menu = Menu::open(id.clone(), options, selected, window, cx);
        let key = id.clone();
        let subscription = cx.subscribe_in(&menu, window, move |this: &mut T, _, event: &MenuEvent, window, cx| {
            let slot = slot(this);
            if let MenuEvent::Dismissed { outside: true } = event {
                slot.closed = Some((key.clone(), Instant::now()));
            }
            slot.close(window, cx);
            if let MenuEvent::Picked(i) = event {
                pick(this, *i, window, cx);
            }
            cx.notify();
        });
        self.open = Some((id, menu, subscription));
    }

    /// Closes the open menu, giving focus back to where it was.
    pub fn close(&mut self, window: &mut Window, cx: &mut App) {
        if let Some((_, menu, _)) = self.open.take() {
            let (focus, restore) = { let m = menu.read(cx); (m.focus.clone(), m.restore.clone()) };
            if focus.is_focused(window) {
                if let Some(restore) = restore {
                    window.focus(&restore, cx);
                }
            }
        }
    }
}

pub enum MenuEvent {
    /// The option at this index was picked (the menu is done).
    Picked(usize),
    /// Closed without a pick (`outside`: by a click elsewhere).
    Dismissed { outside: bool },
}

pub struct Menu {
    id: SharedString,
    options: Vec<SharedString>,
    highlight: usize,
    focus: FocusHandle,
    /// Where focus was before the menu took it.
    restore: Option<FocusHandle>,
}

impl EventEmitter<MenuEvent> for Menu {}

impl Menu {
    /// Opens with `selected` highlighted, and takes focus.
    pub fn open(id: impl Into<SharedString>, options: Vec<SharedString>, selected: Option<usize>, window: &mut Window, cx: &mut App) -> Entity<Menu> {
        let restore = window.focused(cx);
        let menu = cx.new(|cx| Menu {
            id: id.into(),
            highlight: selected.unwrap_or(0).min(options.len().saturating_sub(1)),
            options,
            focus: cx.focus_handle(),
            restore,
        });
        let focus = menu.read(cx).focus.clone();
        window.focus(&focus, cx);
        menu
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = self.options.len().saturating_sub(1) as isize;
        self.highlight = (self.highlight as isize + delta).clamp(0, last) as usize;
        cx.notify();
    }

    #[cfg(test)]
    pub fn highlight(&self) -> usize {
        self.highlight
    }
}

impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Menu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx).clone();
        let id = self.id.clone();
        div()
            .id(SharedString::from(format!("{id}-menu")))
            .key_context("Menu")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &MenuUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &MenuDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &MenuPick, _, cx| cx.emit(MenuEvent::Picked(this.highlight))))
            .on_action(cx.listener(|_, _: &MenuClose, _, cx| cx.emit(MenuEvent::Dismissed { outside: false })))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(MenuEvent::Dismissed { outside: true })))
            .occlude()
            .min_w(u(140.))
            .max_h(u(300.))
            .overflow_y_scroll()
            .rounded(u(6.))
            .border_1()
            .border_color(p.floating_border.unwrap_or(p.border))
            .bg(p.bg_elevated)
            .when(p.shadows, |d| d.shadow_md())
            .py(u(4.))
            .text_size(u(12.))
            .text_color(p.fg)
            .children(self.options.iter().enumerate().map(|(i, label)| {
                div()
                    .id(SharedString::from(format!("{id}-{i}")))
                    .px(u(10.))
                    .py(u(3.))
                    .min_w(px(0.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(i == self.highlight, |d| d.bg(p.selection))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.highlight != i {
                            this.highlight = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(MenuEvent::Picked(i))))
                    .child(label.clone())
            }))
    }
}
