//! Cover-color theming of the player view (CONCEPT.md §4.3).
//!
//! The cover's dominant color becomes the accent and a background tint,
//! **only inside the player view** (`.player-view.themed`). Colors are
//! adjusted until they meet WCAG AA contrast (4.5:1).

use std::cell::OnceCell;

use gtk::gdk;

/// WCAG AA for normal text.
const MIN_CONTRAST: f64 = 4.5;
/// Background colors of libadwaita's light and dark styles.
const LIGHT_BG: [f64; 3] = [0.98, 0.98, 0.98];
const DARK_BG: [f64; 3] = [0.133, 0.133, 0.149];

thread_local! {
    static PROVIDER: OnceCell<gtk::CssProvider> = const { OnceCell::new() };
}

/// The CSS for a cover color; `dark` is the current style.
pub fn css(color: [u8; 3], dark: bool) -> String {
    let base = color.map(|c| f64::from(c) / 255.0);
    // Accent background with white text, else with dark text.
    let white = [1.0, 1.0, 1.0];
    let (accent_bg, accent_fg) = if contrast(base, white) >= MIN_CONTRAST {
        (base, "#ffffff")
    } else if contrast(base, [0.0; 3]) >= MIN_CONTRAST + 1.0 {
        // Light colors keep their color, with dark text.
        (base, "rgba(0, 0, 0, 0.87)")
    } else {
        (adjust_until(base, white, false), "#ffffff")
    };
    // Accent text on the view background.
    let view_bg = if dark { DARK_BG } else { LIGHT_BG };
    let accent = adjust_until(base, view_bg, dark);
    format!(
        ".player-view.themed {{
  --accent-bg-color: {bg};
  --accent-fg-color: {accent_fg};
  --accent-color: {accent};
}}
.player-view.themed .player-tint {{
  background-image: linear-gradient(to bottom, alpha({tint}, 0.55), alpha({tint}, 0.18) 55%, alpha({tint}, 0.05));
}}
",
        bg = hex(accent_bg),
        accent = hex(accent),
        tint = hex(base),
    )
}

/// Applies (or with `None` removes) the player theme.
pub fn apply(color: Option<[u8; 3]>, dark: bool) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    PROVIDER.with(|cell| {
        let provider = cell.get_or_init(|| {
            let provider = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            provider
        });
        let css = color.map(|c| css(c, dark)).unwrap_or_default();
        tracing::debug!("Player theme (dark: {dark}): {css}");
        provider.load_from_string(&css);
    });
}

/// Lightens (`lighter`) or darkens `color` until it contrasts with `against`.
fn adjust_until(color: [f64; 3], against: [f64; 3], lighter: bool) -> [f64; 3] {
    let mut current = color;
    for step in 0..=20 {
        if contrast(current, against) >= MIN_CONTRAST {
            return current;
        }
        let amount = f64::from(step + 1) / 20.0;
        current = color.map(|c| {
            if lighter {
                c + (1.0 - c) * amount
            } else {
                c * (1.0 - amount)
            }
        });
    }
    current
}

/// WCAG 2 contrast ratio.
pub fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn luminance(rgb: [f64; 3]) -> f64 {
    let linear = rgb.map(|c| {
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

fn hex(rgb: [f64; 3]) -> String {
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(hex: &str) -> [f64; 3] {
        let c = |i: usize| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
        [c(1), c(3), c(5)]
    }

    fn value<'a>(css: &'a str, name: &str) -> &'a str {
        let start = css.find(&format!("{name}: ")).unwrap() + name.len() + 2;
        &css[start..start + 7]
    }

    #[test]
    fn contrast_ratios() {
        assert!((contrast([0.0; 3], [1.0; 3]) - 21.0).abs() < 0.01);
        assert!((contrast([0.5; 3], [0.5; 3]) - 1.0).abs() < 0.01);
    }

    #[test]
    fn accents_meet_wcag_aa() {
        // Cyberpunk yellow, Sticker green, a dark navy, a pale gray.
        for color in [
            [0xf3, 0xec, 0x01],
            [0x01, 0xc6, 0x5d],
            [0x10, 0x18, 0x40],
            [0xb6, 0xb6, 0xb8],
        ] {
            for dark in [false, true] {
                let css = css(color, dark);
                let accent = parse(value(&css, "--accent-color"));
                let bg = if dark { DARK_BG } else { LIGHT_BG };
                assert!(
                    contrast(accent, bg) >= MIN_CONTRAST,
                    "{color:?} dark={dark}: {css}"
                );
                let accent_bg = parse(value(&css, "--accent-bg-color"));
                let fg = if css.contains("--accent-fg-color: #ffffff") {
                    [1.0; 3]
                } else {
                    [0.0; 3]
                };
                assert!(contrast(accent_bg, fg) >= MIN_CONTRAST, "{color:?}: {css}");
            }
        }
    }
}
