//! `~/.config/bletherneuk/theme.toml`: fonts, colours and sizes for the pages.
//!
//! The service resolves the theme into CSS custom properties. Pages apply them through
//! the CSSOM, so nothing here ever becomes inline CSS text.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Base {
    #[default]
    SoftDark,
    SoftLight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub base: Base,
    pub match_iterm2: bool,
    pub font: Font,
    pub colors: BTreeMap<String, String>,
    pub layout: Layout,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    pub ui: String,
    pub code: String,
    pub size: f64,
    pub line_height: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub density: Density,
    pub radius: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            base: Base::SoftDark,
            match_iterm2: true,
            font: Font::default(),
            colors: BTreeMap::new(),
            layout: Layout::default(),
        }
    }
}

impl Default for Font {
    fn default() -> Self {
        Font {
            ui: "Inter".into(),
            code: "JetBrains Mono".into(),
            size: 13.0,
            line_height: 1.5,
        }
    }
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            density: Density::Comfortable,
            radius: 6,
        }
    }
}

/// Colour names that `[colors]` may set.
pub const COLOR_KEYS: &[&str] = &[
    "background",
    "surface",
    "text",
    "muted",
    "border",
    "accent",
    "needs_you",
    "working",
    "done",
    "idle",
    "diff_added_bg",
    "diff_removed_bg",
];

/// The current iTerm2 profile's colours, as the bridge reports them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ItermColors {
    pub background: String,
    pub foreground: String,
    /// The 16 ANSI colours, black to bright white.
    pub ansi: Vec<String>,
}

impl ItermColors {
    pub fn is_valid(&self) -> bool {
        is_hex_color(&self.background)
            && is_hex_color(&self.foreground)
            && self.ansi.len() == 16
            && self.ansi.iter().all(|c| is_hex_color(c))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub theme: Theme,
    pub warnings: Vec<String>,
}

impl Theme {
    pub fn parse(text: &str) -> Result<Loaded, toml::de::Error> {
        let mut theme: Theme = toml::from_str(text)?;
        let mut warnings = Vec::new();
        theme.colors.retain(|k, v| {
            if !COLOR_KEYS.contains(&k.as_str()) {
                warnings.push(format!("colors: ignoring unknown colour {k:?}"));
                false
            } else if !is_hex_color(v) {
                warnings.push(format!(
                    "colors.{k}: ignoring {v:?}; use #rgb, #rrggbb or #rrggbbaa"
                ));
                false
            } else {
                true
            }
        });
        let defaults = Font::default();
        if !is_font_name(&theme.font.ui) {
            warnings.push(format!("font.ui: ignoring {:?}", theme.font.ui));
            theme.font.ui = defaults.ui;
        }
        if !is_font_name(&theme.font.code) {
            warnings.push(format!("font.code: ignoring {:?}", theme.font.code));
            theme.font.code = defaults.code;
        }
        if !(6.0..=48.0).contains(&theme.font.size) {
            warnings.push(format!("font.size: ignoring {}", theme.font.size));
            theme.font.size = defaults.size;
        }
        if !(1.0..=3.0).contains(&theme.font.line_height) {
            warnings.push(format!(
                "font.line_height: ignoring {}",
                theme.font.line_height
            ));
            theme.font.line_height = defaults.line_height;
        }
        if theme.layout.radius > 32 {
            warnings.push(format!("layout.radius: ignoring {}", theme.layout.radius));
            theme.layout.radius = Layout::default().radius;
        }
        Ok(Loaded { theme, warnings })
    }

    pub fn load(path: &Path) -> anyhow::Result<Loaded> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Theme::parse(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
                theme: Theme::default(),
                warnings: Vec::new(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolves the theme into CSS custom properties (`--name` → value).
    /// Order: the base palette, then iTerm2's colours if `match_iterm2`, then `[colors]`.
    pub fn css_vars(&self, iterm: Option<&ItermColors>) -> BTreeMap<String, String> {
        let mut palette = base_palette(self.base);
        if self.match_iterm2
            && let Some(it) = iterm.filter(|it| it.is_valid())
        {
            palette = palette_from_iterm(it);
        }
        for (k, v) in &self.colors {
            palette.named.insert(k.clone(), v.clone());
        }

        let mut vars = BTreeMap::new();
        for (k, v) in &palette.named {
            vars.insert(format!("--{}", k.replace('_', "-")), v.clone());
        }
        for (i, c) in palette.ansi.iter().enumerate() {
            vars.insert(format!("--ansi-{i}"), c.clone());
        }
        let a = &palette.ansi;
        let n = &palette.named;
        let shiki = [
            ("--shiki-foreground", n["text"].clone()),
            ("--shiki-background", n["background"].clone()),
            ("--shiki-token-constant", a[3].clone()),
            ("--shiki-token-string", a[2].clone()),
            ("--shiki-token-comment", n["muted"].clone()),
            ("--shiki-token-keyword", a[5].clone()),
            ("--shiki-token-parameter", a[11].clone()),
            ("--shiki-token-function", a[4].clone()),
            ("--shiki-token-string-expression", a[6].clone()),
            ("--shiki-token-punctuation", n["text"].clone()),
            ("--shiki-token-link", a[12].clone()),
        ];
        for (k, v) in shiki {
            vars.insert(k.to_string(), v);
        }
        vars.insert(
            "--font-ui".into(),
            font_stack(&self.font.ui, "system-ui, sans-serif"),
        );
        vars.insert(
            "--font-code".into(),
            font_stack(&self.font.code, "ui-monospace, Menlo, monospace"),
        );
        vars.insert("--font-size".into(), format!("{}px", self.font.size));
        vars.insert("--line-height".into(), format!("{}", self.font.line_height));
        vars.insert("--radius".into(), format!("{}px", self.layout.radius));
        let (pad, gap) = match self.layout.density {
            Density::Comfortable => ("10px", "8px"),
            Density::Compact => ("6px", "4px"),
        };
        vars.insert("--pad".into(), pad.into());
        vars.insert("--gap".into(), gap.into());
        vars
    }
}

struct Palette {
    named: BTreeMap<String, String>,
    ansi: Vec<String>,
}

fn base_palette(base: Base) -> Palette {
    // Soft dark follows Catppuccin Macchiato and soft light follows Catppuccin Latte (MIT).
    let (named, ansi): (&[(&str, &str)], [&str; 16]) = match base {
        Base::SoftDark => (
            &[
                ("background", "#24273a"),
                ("surface", "#1e2030"),
                ("text", "#cad3f5"),
                ("muted", "#8087a2"),
                ("border", "#363a4f"),
                ("accent", "#8aadf4"),
                ("needs_you", "#ed8796"),
                ("working", "#eed49f"),
                ("done", "#a6da95"),
                ("idle", "#8087a2"),
                ("diff_added_bg", "#a6da9526"),
                ("diff_removed_bg", "#ed879626"),
            ],
            [
                "#494d64", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6", "#8bd5ca",
                "#b8c0e0", "#5b6078", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6",
                "#8bd5ca", "#a5adcb",
            ],
        ),
        Base::SoftLight => (
            &[
                ("background", "#eff1f5"),
                ("surface", "#e6e9ef"),
                ("text", "#4c4f69"),
                ("muted", "#8c8fa1"),
                ("border", "#ccd0da"),
                ("accent", "#1e66f5"),
                ("needs_you", "#d20f39"),
                ("working", "#df8e1d"),
                ("done", "#40a02b"),
                ("idle", "#8c8fa1"),
                ("diff_added_bg", "#40a02b26"),
                ("diff_removed_bg", "#d20f3926"),
            ],
            [
                "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299",
                "#acb0be", "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb",
                "#179299", "#bcc0cc",
            ],
        ),
    };
    Palette {
        named: named
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ansi: ansi.iter().map(|s| s.to_string()).collect(),
    }
}

fn palette_from_iterm(it: &ItermColors) -> Palette {
    let bg = parse_hex(&it.background).expect("validated");
    let fg = parse_hex(&it.foreground).expect("validated");
    let ansi: Vec<String> = it.ansi.iter().map(|c| normalise_hex(c)).collect();
    let mut named = BTreeMap::new();
    named.insert("background".into(), to_hex(bg));
    named.insert("surface".into(), to_hex(mix(bg, fg, 0.05)));
    named.insert("text".into(), to_hex(fg));
    named.insert("muted".into(), to_hex(mix(fg, bg, 0.4)));
    named.insert("border".into(), to_hex(mix(bg, fg, 0.15)));
    named.insert("accent".into(), ansi[4].clone());
    named.insert("needs_you".into(), ansi[1].clone());
    named.insert("working".into(), ansi[3].clone());
    named.insert("done".into(), ansi[2].clone());
    named.insert("idle".into(), to_hex(mix(fg, bg, 0.4)));
    named.insert("diff_added_bg".into(), format!("{}26", ansi[2]));
    named.insert("diff_removed_bg".into(), format!("{}26", ansi[1]));
    Palette { named, ansi }
}

pub fn is_hex_color(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_font_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

fn font_stack(name: &str, fallback: &str) -> String {
    format!("\"{name}\", {fallback}")
}

type Rgb = (u8, u8, u8);

fn parse_hex(s: &str) -> Option<Rgb> {
    let hex = s.strip_prefix('#')?;
    let expand = |c: char| c.to_digit(16).map(|d| (d * 17) as u8);
    match hex.len() {
        3 => {
            let mut it = hex.chars();
            Some((
                expand(it.next()?)?,
                expand(it.next()?)?,
                expand(it.next()?)?,
            ))
        }
        6 | 8 => Some((
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        )),
        _ => None,
    }
}

fn normalise_hex(s: &str) -> String {
    to_hex(parse_hex(s).expect("validated"))
}

fn to_hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `a` moved towards `b` by `t` (0..1).
fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

pub const STARTER: &str = r##"# bletherneuk theme. Saved changes apply to open pages straight away.
# See the design doc, section 15.

base = "soft-dark"        # built in: "soft-dark", "soft-light"
match_iterm2 = true       # take background, text and the 16 ANSI colours from the current iTerm2 profile

[font]
ui = "Inter"
code = "JetBrains Mono"
size = 13
line_height = 1.5

[colors]                  # anything set here overrides base and match_iterm2
# accent = "#8aadf4"
# needs_you = "#ed8796"
# working = "#eed49f"
# done = "#a6da95"
# diff_added_bg = "#a6da9526"
# diff_removed_bg = "#ed879626"

[layout]
density = "comfortable"   # or "compact"
radius = 6
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn iterm() -> ItermColors {
        ItermColors {
            background: "#000000".into(),
            foreground: "#ffffff".into(),
            ansi: (0..16).map(|i| format!("#0000{:02x}", i)).collect(),
        }
    }

    #[test]
    fn starter_parses_to_defaults() {
        let loaded = Theme::parse(STARTER).unwrap();
        assert_eq!(loaded.theme, Theme::default());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn reads_the_design_example() {
        let text = r##"
            base = "soft-light"
            match_iterm2 = false
            [font]
            ui = "Inter"
            code = "JetBrains Mono"
            size = 14
            line_height = 1.4
            [colors]
            accent = "#8aadf4"
            diff_added_bg = "#a6da9526"
            [layout]
            density = "compact"
            radius = 4
        "##;
        let t = Theme::parse(text).unwrap().theme;
        assert_eq!(t.base, Base::SoftLight);
        assert!(!t.match_iterm2);
        assert_eq!(t.font.size, 14.0);
        assert_eq!(t.font.line_height, 1.4);
        assert_eq!(t.colors["accent"], "#8aadf4");
        assert_eq!(t.layout.density, Density::Compact);
        assert_eq!(t.layout.radius, 4);
    }

    #[test]
    fn bad_colours_and_fonts_are_dropped() {
        let text = r##"
            [font]
            ui = "Inter; } body { display: none"
            [colors]
            accent = "red"
            done = "#12345"
            sparkle = "#ffffff"
            working = "#abc"
        "##;
        let loaded = Theme::parse(text).unwrap();
        assert_eq!(loaded.theme.font.ui, "Inter");
        assert_eq!(loaded.theme.colors.len(), 1);
        assert_eq!(loaded.theme.colors["working"], "#abc");
        assert_eq!(loaded.warnings.len(), 4);
    }

    #[test]
    fn overrides_beat_iterm_which_beats_base() {
        let mut theme = Theme::default();
        theme.colors.insert("accent".into(), "#123456".into());
        let vars = theme.css_vars(Some(&iterm()));
        assert_eq!(vars["--accent"], "#123456");
        assert_eq!(vars["--background"], "#000000");
        assert_eq!(vars["--text"], "#ffffff");
        assert_eq!(vars["--ansi-5"], "#000005");
        assert_eq!(vars["--done"], "#000002");
    }

    #[test]
    fn iterm_colours_are_ignored_when_match_is_off() {
        let theme = Theme {
            match_iterm2: false,
            ..Theme::default()
        };
        let vars = theme.css_vars(Some(&iterm()));
        assert_eq!(vars["--background"], "#24273a");
    }

    #[test]
    fn invalid_iterm_colours_fall_back_to_base() {
        let mut bad = iterm();
        bad.ansi.pop();
        let vars = Theme::default().css_vars(Some(&bad));
        assert_eq!(vars["--background"], "#24273a");
    }

    #[test]
    fn fonts_and_layout_become_vars() {
        let theme = Theme {
            font: Font {
                ui: "Inter".into(),
                code: "Menlo".into(),
                size: 12.0,
                line_height: 1.25,
            },
            layout: Layout {
                density: Density::Compact,
                radius: 3,
            },
            ..Theme::default()
        };
        let vars = theme.css_vars(None);
        assert_eq!(vars["--font-ui"], "\"Inter\", system-ui, sans-serif");
        assert_eq!(
            vars["--font-code"],
            "\"Menlo\", ui-monospace, Menlo, monospace"
        );
        assert_eq!(vars["--font-size"], "12px");
        assert_eq!(vars["--line-height"], "1.25");
        assert_eq!(vars["--radius"], "3px");
        assert_eq!(vars["--pad"], "6px");
    }

    #[test]
    fn every_value_is_safe_css() {
        let vars = Theme::default().css_vars(Some(&iterm()));
        for (k, v) in vars {
            assert!(k.starts_with("--"));
            assert!(!v.contains([';', '{', '}', '<', '>', '\\']), "{k}: {v}");
        }
    }
}
