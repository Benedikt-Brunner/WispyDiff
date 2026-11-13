//! Shared look: sizes (zoomable), the active palette and zoom, and small widgets.
//!
//! Sizes are written as CSS pixels at 100% zoom (`u(13.)`) and become rems: zooming changes the
//! window's rem size, so the whole window scales like the webview did.

use std::borrow::Cow;

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, rems, svg, AnyElement, App, AssetSource, Div, FontWeight, Global, Hsla, IntoElement, Pixels, Rems,
    SharedString, Window,
};

use crate::theme::{Palette, ThemeId};

/// A size given in CSS pixels at 100% zoom.
pub fn u(css_px: f32) -> Rems {
    rems(css_px / 16.0)
}

/// `u(css_px)` in pixels at the current zoom (for layout arithmetic).
pub fn upx(css_px: f32, cx: &App) -> Pixels {
    px(css_px * zoom(cx))
}

// ---------- palette ----------

/// The theme in use (and the one being previewed in the theme picker).
pub struct Looks {
    pub theme: ThemeId,
    pub preview: Option<ThemeId>,
    pub system_dark: bool,
    pub palette: Palette,
    pub zoom: f32,
}

impl Global for Looks {}

pub fn palette(cx: &App) -> &Palette {
    &cx.global::<Looks>().palette
}

pub fn zoom(cx: &App) -> f32 {
    cx.try_global::<Looks>().map_or(1.0, |l| l.zoom)
}

impl Looks {
    pub fn init(theme: ThemeId, system_dark: bool, zoom: f32, cx: &mut App) {
        let palette = Palette::for_theme(theme, system_dark);
        cx.set_global(Looks { theme, preview: None, system_dark, palette, zoom });
        sync_kit(cx);
    }

    /// Shows `preview` instead of the chosen theme (None: the chosen one again).
    pub fn preview(preview: Option<ThemeId>, cx: &mut App) {
        cx.global_mut::<Looks>().preview = preview;
        Self::refresh(cx);
    }

    pub fn set_theme(theme: ThemeId, cx: &mut App) {
        let looks = cx.global_mut::<Looks>();
        looks.theme = theme;
        looks.preview = None;
        Self::refresh(cx);
    }

    pub fn set_system_dark(dark: bool, cx: &mut App) {
        cx.global_mut::<Looks>().system_dark = dark;
        Self::refresh(cx);
    }

    pub fn set_zoom(zoom: f32, cx: &mut App) {
        cx.global_mut::<Looks>().zoom = zoom;
        sync_kit(cx);
        cx.refresh_windows();
    }

    fn refresh(cx: &mut App) {
        let looks = cx.global_mut::<Looks>();
        looks.palette = Palette::for_theme(looks.preview.unwrap_or(looks.theme), looks.system_dark);
        let shown = looks.preview.unwrap_or(looks.theme);
        sync_kit(cx);
        // The native window (macOS: title bar, scrollbars) follows the theme's light / dark.
        let appearance = match shown {
            ThemeId::System => None,
            t if t.is_dark() => Some(gpui_kit::WindowAppearance::Dark),
            _ => Some(gpui_kit::WindowAppearance::Light),
        };
        cx.set_window_appearance(appearance);
        cx.refresh_windows();
    }
}

/// gpui-kit's components (inputs, title bar, popovers, scrollbars) draw with its own theme: keep it
/// in line with the palette and zoom.
fn sync_kit(cx: &mut App) {
    let (p, zoom) = {
        let looks = cx.global::<Looks>();
        (looks.palette.clone(), looks.zoom)
    };
    let font = ui_font(cx);
    let theme = Theme::global_mut(cx);
    theme.mode = if p.dark { ThemeMode::Dark } else { ThemeMode::Light };
    theme.font_size = px(16.0 * zoom);
    if let Some(family) = font {
        theme.font_family = family;
    }
    theme.mono_font_size = px(12.0 * zoom);
    theme.shadow = p.shadows;
    let c = &mut theme.colors;
    c.background = p.bg;
    c.foreground = p.fg;
    c.border = p.border;
    c.input = p.border;
    c.ring = p.accent;
    c.caret = p.fg;
    // Text views draw the selection over the text: it must let it through.
    c.selection = p.accent.opacity(0.3);
    c.muted = p.bg_sidebar;
    c.muted_foreground = p.fg_muted;
    c.accent = p.selection;
    c.accent_foreground = p.fg;
    c.popover = p.bg_elevated;
    c.popover_foreground = p.fg;
    c.primary = p.accent;
    c.primary_hover = p.accent.opacity(0.9);
    c.primary_active = p.accent.opacity(0.8);
    c.primary_foreground = p.bg_elevated;
    c.secondary = p.bg_sidebar;
    c.secondary_hover = p.selection;
    c.secondary_active = p.selection;
    c.secondary_foreground = p.fg;
    c.title_bar = p.bg_sidebar;
    c.title_bar_border = p.border;
    c.list = p.bg;
    c.list_hover = p.selection.opacity(0.6);
    c.list_active = p.selection;
    c.list_active_border = p.accent;
    c.link = p.accent;
    c.link_hover = p.accent;
    c.link_active = p.accent;
    c.scrollbar = Hsla::transparent_black();
    c.scrollbar_thumb = p.fg_muted.opacity(0.35);
    c.scrollbar_thumb_hover = p.fg_muted.opacity(0.55);
    c.overlay = Hsla { a: 0.25, ..gpui_kit::black() };
    c.window_border = p.border;
    c.sidebar = p.bg_sidebar;
    c.sidebar_border = p.border;
    c.sidebar_foreground = p.fg;
    c.tab_bar = p.bg_sidebar;
    c.danger = p.del_fg;
    c.danger_foreground = p.bg_elevated;
    c.success = p.add_fg;
    c.warning = p.pr(1);
    Theme::sync_base(cx);
}

// ---------- fonts ----------

/// Linux: the UI family, one with real bold faces. The system UI font there is often a variable
/// font (Ubuntu Sans), which GPUI draws at a single weight, so bold and headings went missing.
/// macOS keeps the system font.
fn ui_font(cx: &App) -> Option<SharedString> {
    static PICK: std::sync::OnceLock<Option<SharedString>> = std::sync::OnceLock::new();
    if cfg!(target_os = "macos") {
        return None;
    }
    PICK.get_or_init(|| {
        let installed = cx.text_system().all_font_names();
        ["Inter", "Noto Sans", "DejaVu Sans"].into_iter().find(|f| installed.iter().any(|n| n == f)).map(SharedString::from)
    })
    .clone()
}

pub fn mono_family(cx: &App) -> SharedString {
    gpui_kit::component::ActiveTheme::theme(cx).mono_font_family.clone()
}

pub fn sans_family(cx: &App) -> SharedString {
    gpui_kit::component::ActiveTheme::theme(cx).font_family.clone()
}

// ---------- widgets ----------

/// A key cap: `kbd("Esc")`.
pub fn kbd(key: impl Into<SharedString>, cx: &App) -> Div {
    let p = palette(cx);
    div()
        .px(u(5.))
        .rounded(u(4.))
        .border_1()
        .border_color(p.border)
        .bg(p.bg_sidebar)
        .text_size(u(11.))
        .text_color(p.fg_muted)
        .child(key.into())
}

/// A small rounded label.
pub fn badge(text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    div().px(u(6.)).py(u(1.)).rounded(u(8.)).text_size(u(11.)).text_color(fg).bg(bg).child(text.into())
}

/// An "ok" / "warn" / plain badge in the palette's colours.
pub fn status_badge(text: impl Into<SharedString>, kind: BadgeKind, cx: &App) -> Div {
    let p = palette(cx);
    match kind {
        BadgeKind::Plain => badge(text, p.fg_muted, p.bg_sidebar),
        BadgeKind::Ok => badge(text, p.add_fg, p.add_bg),
        BadgeKind::Warn => badge(text, p.pr(1), p.pr(1).opacity(0.14)),
    }
}

#[derive(Clone, Copy)]
pub enum BadgeKind {
    Plain,
    Ok,
    Warn,
}

/// A text button: `button("close")` (style only; add `.id()` and `.on_click()`).
pub fn link_button(label: impl Into<SharedString>, cx: &App) -> Div {
    let p = palette(cx);
    div().text_size(u(12.)).text_color(p.accent).cursor_pointer().child(label.into())
}

/// A bordered button; `primary` fills it with the accent colour.
pub fn button(label: impl Into<SharedString>, primary: bool, cx: &App) -> Div {
    let p = palette(cx);
    let base = div()
        .flex()
        .items_center()
        .gap(u(6.))
        .px(u(10.))
        .py(u(4.))
        .rounded(u(6.))
        .text_size(u(12.))
        .cursor_pointer()
        .child(label.into());
    if primary {
        base.bg(p.accent).text_color(p.bg_elevated).font_weight(FontWeight::MEDIUM)
    } else {
        base.border_1().border_color(p.border).bg(p.bg_elevated).text_color(p.fg)
    }
}

/// The logo mark (inherits the text colour).
pub fn logo_mark(size: f32, color: Hsla) -> impl IntoElement {
    svg().path("wispy/mark.svg").size(u(size)).text_color(color)
}

/// The wordmark (inherits the text colour).
pub fn wordmark(height: f32, color: Hsla) -> impl IntoElement {
    svg().path("wispy/wordmark.svg").h(u(height)).w(u(height * 4.4)).text_color(color)
}

/// A dimmed backdrop over the window, closing on a click outside its child.
pub fn backdrop(child: impl IntoElement, on_close: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let p = palette(cx);
    div()
        .id("backdrop")
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .items_start()
        .pt(u(90.))
        .bg(Hsla { a: if p.dark { 0.45 } else { 0.18 }, ..gpui_kit::black() })
        .occlude()
        .on_mouse_down(gpui_kit::MouseButton::Left, move |_, window, cx| on_close(window, cx))
        .child(child)
        .into_any_element()
}

/// A floating panel (palette, pickers, dialogs): stops clicks reaching the backdrop.
pub fn floating(width: f32, cx: &App) -> gpui_kit::Stateful<Div> {
    let p = palette(cx);
    div()
        .id("floating")
        .w(u(width))
        // Never wider or taller than the window allows.
        .max_w(gpui_kit::relative(0.94))
        .max_h(gpui_kit::relative(0.8))
        .flex()
        .flex_col()
        .rounded(u(10.))
        .border_1()
        .border_color(p.floating_border.unwrap_or(p.border))
        .bg(p.bg_elevated)
        .text_color(p.fg)
        .when(p.shadows, |d| d.shadow_lg())
        .overflow_hidden()
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

// ---------- assets ----------

/// The app's own assets (the logo) plus gpui-kit's icons.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "wispy/mark.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/logo/mark.svg"))),
            "wispy/wordmark.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/logo/wordmark.svg"))),
            "wispy/comment.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/ui/comment.svg"))),
            "wispy/check.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/ui/check.svg"))),
            "wispy/list.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/ui/list.svg"))),
            "wispy/tree.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/ui/tree.svg"))),
            "wispy/filter.svg" => Some(Cow::Borrowed(include_bytes!("../../../assets/ui/filter.svg"))),
            _ => return gpui_kit::assets::Assets.load(path),
        })
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// One of the app's monochrome icons (`assets/ui`), drawn in `color`.
pub fn icon(name: &str, size: f32, color: Hsla) -> gpui_kit::Svg {
    gpui_kit::svg().path(SharedString::from(format!("wispy/{name}.svg"))).size(u(size)).flex_none().text_color(color)
}

/// "⌘K" on macOS, "Ctrl+K" elsewhere.
pub fn key_label(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// "⌘-click" / "Ctrl-click".
pub fn click_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘-click"
    } else {
        "Ctrl-click"
    }
}

// ---------- rendered Markdown / HTML ----------

/// Options for rendering comments and answers: links open in the browser.
pub fn markdown_options(backend: &std::sync::Arc<crate::backend::Backend>, cx: &App) -> crate::markdown::RenderOptions {
    let backend = backend.clone();
    let mut opts = crate::markdown::RenderOptions::new(std::rc::Rc::new(move |url: &str, _: &mut Window, _: &mut App| {
        if let Err(err) = backend.open_url(url.to_string()) {
            eprintln!("{err}");
        }
    }));
    opts.text_size = upx(13., cx);
    opts.mono_font_family = mono_family(cx);
    let p = palette(cx);
    opts.code_fg = Some(crate::diff::rows::blend(p.fg, p.accent, 0.35));
    opts.code_block_bg = Some(p.fg.opacity(0.06));
    opts
}

thread_local! {
    /// Pasted images by id: loaded (None: there's no such image), and the ones still wanted.
    static IMAGES: std::cell::RefCell<(std::collections::HashMap<String, Option<std::sync::Arc<gpui_kit::Image>>>, std::collections::HashSet<String>)> = Default::default();
}

/// Resolves `wispy-image:` ids from the loaded images; unknown ones are queued for `load_images`.
pub fn image_resolver() -> crate::markdown::ImageResolver {
    std::rc::Rc::new(|id: &str| {
        IMAGES.with(|images| {
            let mut images = images.borrow_mut();
            match images.0.get(id) {
                Some(image) => image.clone(),
                None => {
                    images.1.insert(id.to_string());
                    None
                }
            }
        })
    })
}

/// Loads the images the last render asked for (each once), then redraws.
pub fn load_images(backend: &std::sync::Arc<crate::backend::Backend>, cx: &mut App) {
    let wanted: Vec<String> = IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        let wanted: Vec<String> = images.1.drain().collect();
        for id in &wanted {
            images.0.insert(id.clone(), None);
        }
        wanted
    });
    for id in wanted {
        let task = backend.image_bytes(id.clone());
        cx.spawn(async move |cx| {
            let image = task.await.ok().flatten().map(|(content_type, bytes)| {
                let format = match content_type.as_str() {
                    "image/jpeg" => gpui_kit::ImageFormat::Jpeg,
                    "image/gif" => gpui_kit::ImageFormat::Gif,
                    "image/webp" => gpui_kit::ImageFormat::Webp,
                    _ => gpui_kit::ImageFormat::Png,
                };
                std::sync::Arc::new(gpui_kit::Image::from_bytes(format, bytes))
            });
            IMAGES.with(|images| images.borrow_mut().0.insert(id, image));
            let _ = cx.update(|cx| cx.refresh_windows());
        })
        .detach();
    }
}
