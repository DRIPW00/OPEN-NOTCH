//! Your notch settings: which pages show and in what order, the last page you were on, and
//! the options in Settings. Saved to a small text file (%APPDATA%\OpenNotch\settings.ini)
//! so they survive restarts.

use crate::Page;

/// Every page you can arrange (Home is always first and always on).
const ALL: [Page; 5] = [Page::Music, Page::Shelf, Page::Controls, Page::Network, Page::System];

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// Page order after Home.
    pub order: Vec<Page>,
    pub hidden: Vec<Page>,
    /// The page you were on last: the card opens there.
    pub last: Page,
    /// "Always show": never shrink to the thin bar over full-screen or maximised apps.
    pub always_show: bool,
    /// Animation speed: index into SPEEDS.
    pub speed: usize,
    /// How long the pointer rests on the notch (or the full-screen bar) before it opens, ms.
    pub hover_ms: u32,
    /// What opens the notch: index into OPEN_MODES.
    pub open_mode: usize,
    /// How long paused music stays on the closed notch: index into PAUSED_HIDE.
    pub paused_hide: usize,
}

/// Paused music goes back to the clock after: (label, seconds; None = never).
pub const PAUSED_HIDE: [(&str, Option<u32>); 5] = [("Never", None), ("10 s", Some(10)), ("30 s", Some(30)), ("1 min", Some(60)), ("5 min", Some(300))];
const PAUSED_HIDE_DEFAULT: usize = 2;

/// Animation speed choices: (label, how much faster than normal time runs).
pub const SPEEDS: [(&str, f32); 7] = [
    ("Slow", 0.7),
    ("Normal", 1.0),
    ("Fast", 1.4),
    ("Faster", 2.0),
    ("Gotta go faster", 3.0),
    ("Faster faster fasterrrr", 5.0),
    ("Wally West", 10.0),
];
/// What opens the notch: resting the pointer on it, clicking it, or either.
pub const OPEN_MODES: [&str; 3] = ["Hover", "Click", "Both"];
const OPEN_HOVER: usize = 0;
const OPEN_CLICK: usize = 1;

/// The hover delay you can type: 0.15–1.5 s.
pub const HOVER_MIN_MS: u32 = 150;
pub const HOVER_MAX_MS: u32 = 1500;

/// A typed hover delay in seconds ("0.4", ".75", "1") -> milliseconds, kept within
/// 0.15–1.5 s. None if it isn't a number (the old value stays).
pub fn parse_delay(text: &str) -> Option<u32> {
    let secs: f32 = text.trim().trim_end_matches('s').trim().parse().ok()?;
    if !secs.is_finite() {
        return None;
    }
    Some(((secs * 1000.0).round() as i64).clamp(HOVER_MIN_MS as i64, HOVER_MAX_MS as i64) as u32)
}

/// Milliseconds as the box shows them: "0.3", "0.75", "1.5".
pub fn delay_text(ms: u32) -> String {
    let text = format!("{:.3}", ms as f32 / 1000.0);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A choice index from the file; anything unreadable or out of range is the default.
fn choice(v: &str, count: usize, default: usize) -> usize {
    v.trim().parse::<usize>().ok().filter(|i| *i < count).unwrap_or(default)
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { order: ALL.to_vec(), hidden: Vec::new(), last: Page::Home, always_show: false, speed: 1, hover_ms: 300, open_mode: OPEN_HOVER, paused_hide: PAUSED_HIDE_DEFAULT }
    }
}

pub fn name(page: Page) -> &'static str {
    match page {
        Page::Home => "Home",
        Page::Music => "Music",
        Page::Shelf => "File shelf",
        Page::Controls => "Controls",
        Page::Network => "Network",
        Page::System => "System",
    }
}

fn key(page: Page) -> &'static str {
    match page {
        Page::Home => "home",
        Page::Music => "music",
        Page::Shelf => "shelf",
        Page::Controls => "controls",
        Page::Network => "network",
        Page::System => "system",
    }
}

fn from_key(text: &str) -> Option<Page> {
    ALL.iter().copied().find(|p| key(*p) == text.trim())
}

fn path() -> Option<std::path::PathBuf> {
    Some(settings_in(std::path::Path::new(&std::env::var_os("APPDATA")?)))
}

/// <base>\OpenNotch\settings.ini. Up to R1.03 the app was called Dynamic Island and saved to
/// <base>\DynamicIsland: copy that file over once so nobody loses their settings.
fn settings_in(base: &std::path::Path) -> std::path::PathBuf {
    let new = base.join("OpenNotch").join("settings.ini");
    let old = base.join("DynamicIsland").join("settings.ini");
    if !new.exists() && old.exists() {
        let _ = std::fs::create_dir_all(base.join("OpenNotch"));
        let _ = std::fs::copy(&old, &new);
    }
    new
}

impl Prefs {
    pub fn load() -> Prefs {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|text| Prefs::parse(&text)).unwrap_or_default()
    }

    fn parse(text: &str) -> Prefs {
        let mut prefs = Prefs { order: Vec::new(), ..Prefs::default() };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let pages: Vec<Page> = v.split(',').filter_map(from_key).collect();
            match k.trim() {
                "order" => prefs.order = pages,
                "hidden" => prefs.hidden = pages,
                "last" => prefs.last = pages.first().copied().unwrap_or(Page::Home),
                "always_show" => prefs.always_show = v.trim() == "1",
                "speed" => prefs.speed = choice(v, SPEEDS.len(), 1),
                "hover_ms" => {
                    if let Ok(ms) = v.trim().parse::<u32>() {
                        prefs.hover_ms = ms.clamp(HOVER_MIN_MS, HOVER_MAX_MS);
                    }
                }
                "open" => prefs.open_mode = choice(v, OPEN_MODES.len(), OPEN_HOVER),
                "paused_hide" => prefs.paused_hide = choice(v, PAUSED_HIDE.len(), PAUSED_HIDE_DEFAULT),
                _ => {}
            }
        }
        // Keep the order valid: no repeats, and any page missing from it goes at the end.
        let mut order: Vec<Page> = Vec::new();
        for p in prefs.order.into_iter().chain(ALL) {
            if !order.contains(&p) {
                order.push(p);
            }
        }
        prefs.order = order;
        prefs
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.text());
    }

    fn text(&self) -> String {
        let list = |pages: &[Page]| pages.iter().map(|p| key(*p)).collect::<Vec<_>>().join(",");
        format!(
            "order={}\nhidden={}\nlast={}\nalways_show={}\nspeed={}\nhover_ms={}\nopen={}\npaused_hide={}\n",
            list(&self.order),
            list(&self.hidden),
            key(self.last),
            self.always_show as u8,
            self.speed,
            self.hover_ms,
            self.open_mode,
            self.paused_hide
        )
    }

    /// Resting the pointer on the notch opens it (Hover or Both).
    pub fn hover_opens(&self) -> bool {
        self.open_mode != OPEN_CLICK
    }

    /// Clicking the notch opens it (Click or Both).
    pub fn click_opens(&self) -> bool {
        self.open_mode != OPEN_HOVER
    }

    pub fn toggle(&mut self, page: Page) {
        if self.hidden.contains(&page) {
            self.hidden.retain(|p| *p != page);
        } else {
            self.hidden.push(page);
        }
    }

    /// Move the page at `from` to position `to`.
    pub fn reorder(&mut self, from: usize, to: usize) {
        if from < self.order.len() && to < self.order.len() && from != to {
            let page = self.order.remove(from);
            self.order.insert(to, page);
        }
    }
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
