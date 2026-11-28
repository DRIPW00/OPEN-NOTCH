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
mod tests {
    use super::*;

    // R1.04: Settings > Paused music, default 30 s, survives a restart.
    #[test]
    fn paused_music_setting() {
        assert_eq!(PAUSED_HIDE[Prefs::default().paused_hide].1, Some(30));
        let p = Prefs { paused_hide: 0, ..Prefs::default() };
        assert_eq!(Prefs::parse(&p.text()).paused_hide, 0, "Never is kept");
        assert_eq!(Prefs::parse("paused_hide=99\n").paused_hide, Prefs::default().paused_hide, "nonsense: default");
    }

    // Rename: settings saved under the old name are carried over once, then left alone.
    #[test]
    fn old_settings_carry_over_to_opennotch() {
        let base = std::env::temp_dir().join(format!("opennotch-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("DynamicIsland")).unwrap();
        std::fs::write(base.join("DynamicIsland").join("settings.ini"), "speed=fast").unwrap();

        let new = settings_in(&base);
        assert_eq!(new, base.join("OpenNotch").join("settings.ini"));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "speed=fast");

        // Once the new file exists, the old one never overwrites it.
        std::fs::write(&new, "speed=slow").unwrap();
        settings_in(&base);
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "speed=slow");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parse_fills_in_missing_pages() {
        let p = Prefs::parse("order=system,controls\nhidden=music\n");
        assert_eq!(p.order, vec![Page::System, Page::Controls, Page::Music, Page::Shelf, Page::Network]);
        assert_eq!(p.hidden, vec![Page::Music]);
    }

    #[test]
    fn hidden_page_listed_twice_can_be_shown_again() {
        let mut p = Prefs::parse("hidden=music,shelf,music\n");
        p.toggle(Page::Music);
        assert_eq!(p.hidden, vec![Page::Shelf]);
    }

    // F3: the last page survives a restart; Home when there is none.
    #[test]
    fn last_page_round_trips() {
        let p = Prefs { last: Page::Network, ..Prefs::default() };
        assert_eq!(Prefs::parse(&p.text()).last, Page::Network);
    }

    #[test]
    fn last_page_defaults_to_home() {
        assert_eq!(Prefs::parse("order=system\n").last, Page::Home);
        assert_eq!(Prefs::parse("last=nonsense\n").last, Page::Home);
        assert_eq!(Prefs::parse("last=home\n").last, Page::Home);
    }

    #[test]
    fn old_settings_file_still_loads() {
        // An R1.00 file has no new keys: everything else keeps its default.
        let p = Prefs::parse("order=system,controls,music,shelf,network\nhidden=shelf\n");
        assert_eq!(p.hidden, vec![Page::Shelf]);
        assert_eq!(p.last, Page::Home);
    }

    // Always show survives a restart; anything but "1" is off.
    #[test]
    fn always_show_round_trips() {
        let p = Prefs { always_show: true, ..Prefs::default() };
        assert!(Prefs::parse(&p.text()).always_show);
        assert!(!Prefs::parse(&Prefs::default().text()).always_show);
        assert!(!Prefs::parse("always_show=yes\n").always_show);
        // The removed Always hide never hides anything now.
        assert!(!Prefs::parse("always_hide=1\n").text().contains("always_hide"));
    }

    // F6: animation speed and hover delay survive a restart; bad values fall back.
    #[test]
    fn motion_choices_round_trip() {
        let p = Prefs { speed: 2, hover_ms: 850, ..Prefs::default() };
        let q = Prefs::parse(&p.text());
        assert_eq!((q.speed, q.hover_ms), (2, 850));
    }

    #[test]
    fn motion_choices_reject_bad_values() {
        let q = Prefs::parse("speed=9\nhover_ms=-1\n");
        assert_eq!((q.speed, q.hover_ms), (1, 300));
        assert_eq!(Prefs::parse("hover_ms=99999\n").hover_ms, HOVER_MAX_MS);
        assert_eq!(Prefs::parse("hover_ms=5\n").hover_ms, HOVER_MIN_MS);
        let q = Prefs::parse("speed=abc\n");
        assert_eq!(q.speed, 1);
    }

    #[test]
    fn default_speed_and_delay() {
        let p = Prefs::default();
        assert_eq!(SPEEDS[p.speed].1, 1.0); // normal speed
        assert_eq!(p.hover_ms, 300);
    }

    // Test builds saved settings that are gone now ("bounce", "watch", …): they're ignored.
    #[test]
    fn removed_bounce_setting_is_ignored() {
        let p = Prefs::parse("order=system\nhidden=\nlast=controls\nalways_hide=0\nspeed=1\nhover_delay=3\nbounce=3\nwatch=1\n");
        assert_eq!((p.last, p.hover_ms), (Page::Controls, 300));
        for removed in ["bounce", "hover_delay", "watch", "always_hide"] {
            assert!(!p.text().contains(removed), "{removed}");
        }
    }

    // The hover delay box: what you type, and what it shows.
    #[test]
    fn typed_delay_is_read_in_seconds_and_kept_in_range() {
        assert_eq!(parse_delay("0.4"), Some(400));
        assert_eq!(parse_delay(".75"), Some(750));
        assert_eq!(parse_delay("1"), Some(1000));
        assert_eq!(parse_delay(" 0.5 s "), Some(500));
        assert_eq!(parse_delay("0.15"), Some(150));
        assert_eq!(parse_delay("1.5"), Some(1500));
        assert_eq!(parse_delay("0.01"), Some(HOVER_MIN_MS)); // too short: 0.15
        assert_eq!(parse_delay("9"), Some(HOVER_MAX_MS)); // too long: 1.5
        assert_eq!(parse_delay(""), None);
        assert_eq!(parse_delay("."), None);
        assert_eq!(parse_delay("abc"), None);
        assert_eq!(parse_delay("inf"), None);
    }

    #[test]
    fn delay_shows_without_trailing_zeros() {
        assert_eq!(delay_text(300), "0.3");
        assert_eq!(delay_text(750), "0.75");
        assert_eq!(delay_text(1500), "1.5");
        assert_eq!(delay_text(1000), "1");
        assert_eq!(parse_delay(&delay_text(850)), Some(850)); // what it shows reads back the same
    }

    // Bug hunt B2: every allowed delay shows the exact value it uses.
    #[test]
    fn every_delay_reads_back_exactly() {
        for ms in HOVER_MIN_MS..=HOVER_MAX_MS {
            assert_eq!(parse_delay(&delay_text(ms)), Some(ms), "{ms} ms shows as {}", delay_text(ms));
        }
    }

    #[test]
    fn delay_rejects_nan_and_negative() {
        assert_eq!(parse_delay("NaN"), None);
        assert_eq!(parse_delay("-1"), Some(HOVER_MIN_MS));
    }

    #[test]
    fn reorder_ignores_bad_positions() {
        let mut p = Prefs::default();
        p.reorder(0, 99);
        p.reorder(99, 0);
        assert_eq!(p.order, Prefs::default().order);
        p.reorder(0, 4);
        assert_eq!(p.order.last(), Some(&Page::Music));
    }

    // Open on: Hover / Click / Both.
    #[test]
    fn open_modes_decide_hover_and_click() {
        let with = |open_mode| Prefs { open_mode, ..Prefs::default() };
        assert_eq!(Prefs::default().open_mode, OPEN_HOVER); // as before: hover opens it
        let (hover, click, both) = (with(0), with(1), with(2));
        assert!(hover.hover_opens() && !hover.click_opens());
        assert!(!click.hover_opens() && click.click_opens());
        assert!(both.hover_opens() && both.click_opens());
        assert_eq!(Prefs::parse(&both.text()).open_mode, 2);
        assert_eq!(Prefs::parse("open=7\n").open_mode, OPEN_HOVER);
    }
}
