//! Colour palettes (ported from the React app's CSS tokens; now the source of truth).
use gpui_kit::{rgb, FontStyle, FontWeight, Hsla};

/// Persisted theme IDs match the React client; System is resolved by the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ThemeId {
    #[default]
    System,
    Light,
    Dark,
    SolarizedLight,
    SolarizedDark,
    Nord,
    Dracula,
    Sepia,
    Eink,
}
impl ThemeId {
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::SolarizedLight => "solarized-light",
            Self::SolarizedDark => "solarized-dark",
            Self::Nord => "nord",
            Self::Dracula => "dracula",
            Self::Sepia => "sepia",
            Self::Eink => "eink",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System (light / dark)",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::SolarizedLight => "Solarized Light",
            Self::SolarizedDark => "Solarized Dark",
            Self::Nord => "Nord",
            Self::Dracula => "Dracula",
            Self::Sepia => "Sepia",
            Self::Eink => "E-Ink",
        }
    }
    pub fn all() -> &'static [Self] {
        &[
            Self::System,
            Self::Light,
            Self::Dark,
            Self::SolarizedLight,
            Self::SolarizedDark,
            Self::Nord,
            Self::Dracula,
            Self::Sepia,
            Self::Eink,
        ]
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::all().iter().copied().find(|t| t.id() == id)
    }
    /// Intrinsically dark themes. For System, call `resolve(system_dark).is_dark()`.
    pub fn is_dark(self) -> bool {
        matches!(
            self,
            Self::Dark | Self::SolarizedDark | Self::Nord | Self::Dracula
        )
    }
    pub fn resolve(self, system_dark: bool) -> Self {
        if self == Self::System {
            if system_dark {
                Self::Dark
            } else {
                Self::Light
            }
        } else {
            self
        }
    }
}
#[derive(Clone)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub bg_elevated: Hsla,
    pub bg_sidebar: Hsla,
    pub fg: Hsla,
    pub fg_muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub add_bg: Hsla,
    pub add_gutter: Hsla,
    pub add_fg: Hsla,
    pub add_word: Hsla,
    pub del_bg: Hsla,
    pub del_gutter: Hsla,
    pub del_fg: Hsla,
    pub del_word: Hsla,
    pub hunk_bg: Hsla,
    pub hunk_fg: Hsla,
    pub file_bg: Hsla,
    pub selection: Hsla,
    /// Indexed by `wispy_core::highlight::TOKEN_CLASSES`; 0 is plain text.
    pub tokens: [Hsla; 20],
    /// Per-PR colours, cycled by stack index.
    pub prs: [Hsla; 6],
    pub eink: bool,
    /// E-Ink panels ghost on animation: jumps land immediately.
    pub instant_jumps: bool,
    pub shadows: bool,
    /// E-Ink keywords/tags are bold, types semibold; other classes remain normal.
    pub token_weights: [FontWeight; 20],
    /// Comments remain italic in every theme.
    pub token_styles: [FontStyle; 20],
    /// Deleted-line numbers invert to white on E-Ink's black gutter.
    pub del_gutter_fg: Hsla,
    /// A hairline at the top of hunk rows (including their gutters), inside row geometry.
    pub hunk_top_border: Option<Hsla>,
    /// E-Ink frames floating palettes, sheets, toasts, file views, cards and filter menus.
    pub floating_border: Option<Hsla>,
}
fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}
/// CSS `color-mix(in srgb, color 22%, transparent)`.
fn word(color: Hsla) -> Hsla {
    Hsla { a: 0.22, ..color }
}
fn weights(eink: bool) -> [FontWeight; 20] {
    let mut weights = [FontWeight::NORMAL; 20];
    if eink {
        for i in [1, 14] {
            weights[i] = FontWeight::BOLD;
        }
        for i in [7, 9] {
            weights[i] = FontWeight::SEMIBOLD;
        }
    }
    weights
}
fn styles() -> [FontStyle; 20] {
    let mut styles = [FontStyle::Normal; 20];
    styles[3] = FontStyle::Italic;
    styles
}
impl Palette {
    pub fn for_theme(theme: ThemeId, system_dark: bool) -> Self {
        match theme.resolve(system_dark) {
            ThemeId::System => unreachable!("System is resolved"),
            ThemeId::Light => Self::light(),
            ThemeId::Dark => Self::dark(),
            ThemeId::SolarizedLight => Self::solarized_light(),
            ThemeId::SolarizedDark => Self::solarized_dark(),
            ThemeId::Nord => Self::nord(),
            ThemeId::Dracula => Self::dracula(),
            ThemeId::Sepia => Self::sepia(),
            ThemeId::Eink => Self::eink(),
        }
    }
    pub fn light() -> Self {
        Self {
            dark: false,
            bg: c(0xfbfbfa),
            bg_elevated: c(0xffffff),
            bg_sidebar: c(0xf4f4f2),
            fg: c(0x1f2328),
            fg_muted: c(0x6e7781),
            border: c(0xe3e3e0),
            accent: c(0x5b6cf0),
            add_bg: c(0xe9f7ee),
            add_gutter: c(0xd3f0dc),
            add_fg: c(0x1a7f37),
            add_word: word(c(0x1a7f37)),
            del_bg: c(0xfcecee),
            del_gutter: c(0xf8d7db),
            del_fg: c(0xcf222e),
            del_word: word(c(0xcf222e)),
            hunk_bg: c(0xf1f3fb),
            hunk_fg: c(0x6a73a8),
            file_bg: c(0xf6f6f4),
            selection: c(0xdbe3ff),
            tokens: [
                c(0x1f2328),
                c(0xa626a4),
                c(0x3f7f3a),
                c(0x8a9199),
                c(0xb25d00),
                c(0xb25d00),
                c(0x3057c9),
                c(0xa0620a),
                c(0xb25d00),
                c(0xa0620a),
                c(0x1e6f8a),
                c(0x1f2328),
                c(0x5c6370),
                c(0x6e7781),
                c(0xc4304d),
                c(0x9a6700),
                c(0x0f7b6c),
                c(0xa0620a),
                c(0xa0620a),
                c(0xa0620a),
            ],
            prs: [
                c(0x5b6cf0),
                c(0xd4852a),
                c(0x1f9aa5),
                c(0xb9509e),
                c(0x7d8a1d),
                c(0x8a63d2),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xcf222e),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: c(0x16171a),
            bg_elevated: c(0x1e2024),
            bg_sidebar: c(0x1a1b1e),
            fg: c(0xd8dadf),
            fg_muted: c(0x7d828c),
            border: c(0x2a2c31),
            accent: c(0x8b97ff),
            add_bg: c(0x15261c),
            add_gutter: c(0x1b3324),
            add_fg: c(0x56d364),
            add_word: word(c(0x56d364)),
            del_bg: c(0x2c1719),
            del_gutter: c(0x3d1d21),
            del_fg: c(0xf47067),
            del_word: word(c(0xf47067)),
            hunk_bg: c(0x1c1f2b),
            hunk_fg: c(0x8790c4),
            file_bg: c(0x1d1f23),
            selection: c(0x2c3558),
            tokens: [
                c(0xd8dadf),
                c(0xd197ea),
                c(0x9ccf84),
                c(0x6b717c),
                c(0xe5a36b),
                c(0xe5a36b),
                c(0x82aaff),
                c(0xe8c47a),
                c(0xe5a36b),
                c(0xe8c47a),
                c(0x7fcbe0),
                c(0xd8dadf),
                c(0xa0a6b1),
                c(0x8b919c),
                c(0xf07178),
                c(0xe8c47a),
                c(0x6ed4c4),
                c(0xe8c47a),
                c(0xe8c47a),
                c(0xe8c47a),
            ],
            prs: [
                c(0x8b97ff),
                c(0xf0a44b),
                c(0x4cc7d1),
                c(0xe07cc7),
                c(0xb6c24a),
                c(0xb08cf5),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xf47067),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn solarized_light() -> Self {
        Self {
            dark: false,
            bg: c(0xfdf6e3),
            bg_elevated: c(0xfffbf0),
            bg_sidebar: c(0xf5eedb),
            fg: c(0x586e75),
            fg_muted: c(0x93a1a1),
            border: c(0xe6dfc8),
            accent: c(0x268bd2),
            add_bg: c(0xeef3d6),
            add_gutter: c(0xdfe8b8),
            add_fg: c(0x6c7d00),
            add_word: word(c(0x6c7d00)),
            del_bg: c(0xfbe4dc),
            del_gutter: c(0xf5cfc3),
            del_fg: c(0xdc322f),
            del_word: word(c(0xdc322f)),
            hunk_bg: c(0xeee8d5),
            hunk_fg: c(0x6c71c4),
            file_bg: c(0xf5eedb),
            selection: c(0xdde6ee),
            tokens: [
                c(0x586e75),
                c(0x859900),
                c(0x2aa198),
                c(0x93a1a1),
                c(0xd33682),
                c(0xd33682),
                c(0x268bd2),
                c(0xb58900),
                c(0xcb4b16),
                c(0xb58900),
                c(0x6c71c4),
                c(0x586e75),
                c(0x657b83),
                c(0x93a1a1),
                c(0x268bd2),
                c(0xb58900),
                c(0xcb4b16),
                c(0xb58900),
                c(0xb58900),
                c(0xb58900),
            ],
            prs: [
                c(0x6c71c4),
                c(0xcb4b16),
                c(0x2aa198),
                c(0xd33682),
                c(0xb58900),
                c(0x268bd2),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xdc322f),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn solarized_dark() -> Self {
        Self {
            dark: true,
            bg: c(0x002b36),
            bg_elevated: c(0x073642),
            bg_sidebar: c(0x01313d),
            fg: c(0x93a1a1),
            fg_muted: c(0x657b83),
            border: c(0x0d3f4b),
            accent: c(0x268bd2),
            add_bg: c(0x0f3a2c),
            add_gutter: c(0x164a37),
            add_fg: c(0x859900),
            add_word: word(c(0x859900)),
            del_bg: c(0x3a1f28),
            del_gutter: c(0x4a2530),
            del_fg: c(0xdc322f),
            del_word: word(c(0xdc322f)),
            hunk_bg: c(0x073642),
            hunk_fg: c(0x6c71c4),
            file_bg: c(0x073642),
            selection: c(0x174956),
            tokens: [
                c(0x93a1a1),
                c(0x859900),
                c(0x2aa198),
                c(0x586e75),
                c(0xd33682),
                c(0xd33682),
                c(0x268bd2),
                c(0xb58900),
                c(0xcb4b16),
                c(0xb58900),
                c(0x6c71c4),
                c(0x93a1a1),
                c(0x839496),
                c(0x657b83),
                c(0x268bd2),
                c(0xb58900),
                c(0xcb4b16),
                c(0xb58900),
                c(0xb58900),
                c(0xb58900),
            ],
            prs: [
                c(0x6c71c4),
                c(0xcb4b16),
                c(0x2aa198),
                c(0xd33682),
                c(0xb58900),
                c(0x268bd2),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xdc322f),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn nord() -> Self {
        Self {
            dark: true,
            bg: c(0x2e3440),
            bg_elevated: c(0x3b4252),
            bg_sidebar: c(0x2b303b),
            fg: c(0xd8dee9),
            fg_muted: c(0x7b88a1),
            border: c(0x3b4252),
            accent: c(0x88c0d0),
            add_bg: c(0x384841),
            add_gutter: c(0x43574d),
            add_fg: c(0xa3be8c),
            add_word: word(c(0xa3be8c)),
            del_bg: c(0x463a42),
            del_gutter: c(0x574149),
            del_fg: c(0xbf616a),
            del_word: word(c(0xbf616a)),
            hunk_bg: c(0x353c4a),
            hunk_fg: c(0x81a1c1),
            file_bg: c(0x3b4252),
            selection: c(0x434c5e),
            tokens: [
                c(0xd8dee9),
                c(0x81a1c1),
                c(0xa3be8c),
                c(0x616e88),
                c(0xb48ead),
                c(0xb48ead),
                c(0x88c0d0),
                c(0x8fbcbb),
                c(0xb48ead),
                c(0x8fbcbb),
                c(0xd8dee9),
                c(0xd8dee9),
                c(0x81a1c1),
                c(0xa0a8b7),
                c(0x81a1c1),
                c(0x8fbcbb),
                c(0xebcb8b),
                c(0x8fbcbb),
                c(0x8fbcbb),
                c(0x8fbcbb),
            ],
            prs: [
                c(0x88c0d0),
                c(0xd08770),
                c(0x8fbcbb),
                c(0xb48ead),
                c(0xebcb8b),
                c(0x5e81ac),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xbf616a),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn dracula() -> Self {
        Self {
            dark: true,
            bg: c(0x282a36),
            bg_elevated: c(0x303241),
            bg_sidebar: c(0x21222c),
            fg: c(0xf8f8f2),
            fg_muted: c(0x7f86ab),
            border: c(0x3a3c4e),
            accent: c(0xbd93f9),
            add_bg: c(0x2a3d36),
            add_gutter: c(0x31503f),
            add_fg: c(0x50fa7b),
            add_word: word(c(0x50fa7b)),
            del_bg: c(0x402a35),
            del_gutter: c(0x55303e),
            del_fg: c(0xff5555),
            del_word: word(c(0xff5555)),
            hunk_bg: c(0x2f3142),
            hunk_fg: c(0xbd93f9),
            file_bg: c(0x303241),
            selection: c(0x44475a),
            tokens: [
                c(0xf8f8f2),
                c(0xff79c6),
                c(0xf1fa8c),
                c(0x6272a4),
                c(0xbd93f9),
                c(0xbd93f9),
                c(0x50fa7b),
                c(0x8be9fd),
                c(0xbd93f9),
                c(0x8be9fd),
                c(0x8be9fd),
                c(0xf8f8f2),
                c(0xff79c6),
                c(0xa4a7c1),
                c(0xff79c6),
                c(0x50fa7b),
                c(0xffb86c),
                c(0x8be9fd),
                c(0x8be9fd),
                c(0x8be9fd),
            ],
            prs: [
                c(0xbd93f9),
                c(0xffb86c),
                c(0x8be9fd),
                c(0xff79c6),
                c(0xf1fa8c),
                c(0x6272a4),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xff5555),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn sepia() -> Self {
        Self {
            dark: false,
            bg: c(0xf4ecd8),
            bg_elevated: c(0xfbf5e6),
            bg_sidebar: c(0xede3cb),
            fg: c(0x433422),
            fg_muted: c(0x8a7a62),
            border: c(0xe0d4b8),
            accent: c(0xa0522d),
            add_bg: c(0xe6edcf),
            add_gutter: c(0xd5e2b3),
            add_fg: c(0x4f7a28),
            add_word: word(c(0x4f7a28)),
            del_bg: c(0xf5dccb),
            del_gutter: c(0xecc6b0),
            del_fg: c(0xb03a2e),
            del_word: word(c(0xb03a2e)),
            hunk_bg: c(0xebe0c6),
            hunk_fg: c(0x7a6a4f),
            file_bg: c(0xede3cb),
            selection: c(0xe3d3ae),
            tokens: [
                c(0x433422),
                c(0x8f3f71),
                c(0x4f7a28),
                c(0xa0917a),
                c(0xb5651d),
                c(0xb5651d),
                c(0x2f5f8a),
                c(0x8a6412),
                c(0xb5651d),
                c(0x8a6412),
                c(0x2d6f6f),
                c(0x433422),
                c(0x6b5a44),
                c(0x8a7a62),
                c(0xa8323e),
                c(0x8a6412),
                c(0x2d6f6f),
                c(0x8a6412),
                c(0x8a6412),
                c(0x8a6412),
            ],
            prs: [
                c(0x2f5f8a),
                c(0xb5651d),
                c(0x2d8a8a),
                c(0x8f3f71),
                c(0x6f7a1d),
                c(0x6a4fa0),
            ],
            eink: false,
            instant_jumps: false,
            shadows: true,
            token_weights: weights(false),
            token_styles: styles(),
            del_gutter_fg: c(0xb03a2e),
            hunk_top_border: None,
            floating_border: None,
        }
    }
    pub fn eink() -> Self {
        Self {
            dark: false,
            bg: c(0xffffff),
            bg_elevated: c(0xffffff),
            bg_sidebar: c(0xffffff),
            fg: c(0x000000),
            fg_muted: c(0x4d4d4d),
            border: c(0x8c8c8c),
            accent: c(0x000000),
            add_bg: c(0xe6e6e6),
            add_gutter: c(0xbfbfbf),
            add_fg: c(0x000000),
            add_word: c(0xbfbfbf),
            del_bg: c(0xffffff),
            del_gutter: c(0x000000),
            del_fg: c(0x000000),
            del_word: c(0xd9d9d9),
            hunk_bg: c(0xffffff),
            hunk_fg: c(0x000000),
            file_bg: c(0xe6e6e6),
            selection: c(0xbfbfbf),
            tokens: [
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x4d4d4d),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x4d4d4d),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
                c(0x000000),
            ],
            prs: [
                c(0x000000),
                c(0x737373),
                c(0x404040),
                c(0xa6a6a6),
                c(0x262626),
                c(0x8c8c8c),
            ],
            eink: true,
            instant_jumps: true,
            shadows: false,
            token_weights: weights(true),
            token_styles: styles(),
            del_gutter_fg: c(0xffffff),
            hunk_top_border: Some(c(0x8c8c8c)),
            floating_border: Some(c(0x000000)),
        }
    }
    pub fn pr(&self, index: u8) -> Hsla {
        self.prs[index as usize % self.prs.len()]
    }

    /// The band between two files in the diff: the background, a shade towards the text.
    pub fn file_gap(&self) -> Hsla {
        let (b, f) = (self.bg.to_rgb(), self.fg.to_rgb());
        let mix = |x: f32, y: f32| x + (y - x) * 0.06;
        gpui_kit::Rgba { r: mix(b.r, f.r), g: mix(b.g, f.g), b: mix(b.b, f.b), a: 1. }.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_names_and_system_resolution() {
        assert_eq!(ThemeId::all().len(), 9);
        for theme in ThemeId::all() {
            assert_eq!(ThemeId::from_id(theme.id()), Some(*theme));
            assert!(!theme.name().is_empty());
            assert_eq!(
                Palette::for_theme(*theme, true).dark,
                theme.resolve(true).is_dark()
            );
        }
        assert_eq!(ThemeId::from_id("unknown"), None);
        assert_eq!(
            Palette::for_theme(ThemeId::System, false).bg,
            Palette::light().bg
        );
        assert_eq!(
            Palette::for_theme(ThemeId::System, true).bg,
            Palette::dark().bg
        );
        assert_eq!(Palette::dark().pr(7), Palette::dark().prs[1]);
    }
    #[test]
    fn eink_preserves_non_colour_policies() {
        let e = Palette::eink();
        assert!(e.eink && e.instant_jumps && !e.shadows);
        assert_eq!(e.add_word, c(0xbfbfbf));
        assert_eq!(e.del_word, c(0xd9d9d9));
        assert_eq!(e.del_gutter_fg, c(0xffffff));
        assert_eq!(e.hunk_top_border, Some(e.border));
        assert_eq!(e.floating_border, Some(c(0)));
        for i in 0..20 {
            assert_eq!(
                e.token_weights[i],
                match i {
                    1 | 14 => FontWeight::BOLD,
                    7 | 9 => FontWeight::SEMIBOLD,
                    _ => FontWeight::NORMAL,
                }
            );
            assert_eq!(
                e.token_styles[i],
                if i == 3 {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                }
            );
        }
        for t in ThemeId::all()
            .iter()
            .copied()
            .filter(|t| *t != ThemeId::Eink)
        {
            let p = Palette::for_theme(t, false);
            assert!(p.shadows && !p.instant_jumps && !p.eink);
            assert!(p.hunk_top_border.is_none());
        }
    }
}
