//! Now playing on the notch: which app's music is shown, the closed notch's music state
//! (paused timeout, new-song pop-up) and song times.

use super::*;

/// The new-song pop-up: true when `now` (the shown song, only while it plays) is a song not
/// announced before. Pausing doesn't forget it, so the same song resuming stays quiet.
pub(crate) fn announce(last: &mut Option<String>, now: Option<String>) -> bool {
    match now {
        Some(song) if last.as_ref() != Some(&song) => {
            *last = Some(song);
            true
        }
        _ => false,
    }
}

/// Settings > Paused music: paused this long, the closed notch goes back to the clock.
pub(crate) fn paused_out(paused_for: Option<Duration>, limit_secs: Option<u32>) -> bool {
    matches!((paused_for, limit_secs), (Some(d), Some(limit)) if d >= Duration::from_secs(limit as u64))
}

/// Time left in the song, as music apps show it: "-2:30".
pub(crate) fn time_left(position: f64, duration: f64) -> String {
    format!("-{}", format_time(duration - position))
}

/// Which media session the notch shows (and sends commands to). The one you picked stays
/// until it stops or goes away: a new app starting to play, or Windows' "current" app
/// changing, doesn't switch it (the source tabs show those). With none, it picks what's
/// playing (Windows' current first), else what's paused.
pub(crate) fn pick_source(selected: Option<&str>, sessions: &[(&str, Status)], system: Option<&str>) -> Option<String> {
    let is = |id: &str, ok: &dyn Fn(Status) -> bool| sessions.iter().any(|(s, st)| *s == id && ok(*st));
    let live = |st: Status| st != Status::Stopped;
    let playing = |st: Status| st == Status::Playing;
    if let Some(id) = selected.filter(|id| is(id, &live)) {
        return Some(id.to_string());
    }
    let first = |ok: &dyn Fn(Status) -> bool| sessions.iter().find(|(_, st)| ok(*st)).map(|(id, _)| *id);
    system
        .filter(|id| is(id, &playing))
        .or_else(|| first(&playing))
        .or(system.filter(|id| is(id, &live)))
        .or_else(|| first(&live))
        .map(str::to_string)
}

pub(crate) fn format_time(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl App {

    pub(crate) fn current(&self) -> Option<&Session> {
        let id = self.selected.as_ref()?;
        self.sessions.iter().find(|s| &s.id == id)
    }

    pub(crate) fn has_media(&self) -> bool {
        self.current().is_some_and(|s| s.status != Status::Stopped)
    }

    /// Tabs: the shown session first, then every other app with media loaded (greyed out),
    /// playing ones before paused ones.
    pub(crate) fn tabs(&self) -> Vec<&Session> {
        let mut tabs: Vec<&Session> = self.current().into_iter().collect();
        let others = |status: Status| {
            self.sessions
                .iter()
                .filter(move |s| s.status == status && Some(&s.id) != self.selected.as_ref())
        };
        tabs.extend(others(Status::Playing));
        tabs.extend(others(Status::Paused));
        tabs
    }

    /// A new picture of what's playing arrived from media.rs. Which session it shows: see pick_source.
    pub(crate) unsafe fn apply_snapshot(&mut self, snapshot: MediaSnapshot) {
        let had_media = self.has_media();
        let page_before = self.page_kind();
        if let Some(notice) = snapshot.notice {
            self.notice = Some(notice);
            SetTimer(Some(self.hwnd), NOTICE_TIMER, NOTICE_MS, None);
        }
        self.sessions = snapshot.sessions;

        let ids: Vec<(&str, Status)> = self.sessions.iter().map(|s| (s.id.as_str(), s.status)).collect();
        self.selected = pick_source(self.selected.as_deref(), &ids, snapshot.system_current.as_deref());

        // Upload album art for the shown session; forget art no longer needed.
        let wanted = self.current().and_then(|s| s.art.clone());
        self.art.retain(|key, _| wanted.as_ref().is_some_and(|a| a.key == *key));
        if let Some(art) = wanted {
            if !self.art.contains_key(&art.key) {
                let t: ID2D1RenderTarget = self.target.clone().into();
                if let (Ok(big), Ok(small)) = (
                    gfx::bitmap(&t, &art.big, art.big_size),
                    gfx::bitmap(&t, &art.small, art.small_size),
                ) {
                    self.art.insert(art.key, (big, small));
                }
            }
        }


        // The Music page comes and goes with media: stay on the same page if it still exists.
        match self.pages().iter().position(|p| *p == page_before) {
            Some(i) => {
                if i != self.page {
                    self.page = i;
                    self.page_pos = Ease::new(i as f32);
                }
                if had_media != self.has_media() {
                    self.set_expanded(self.hovered);
                }
            }
            None => self.switch_page(0), // the Music page went away
        }
        // New-song pop-up on the closed notch (not for the song already playing at launch).
        let now = self.current().filter(|s| s.status == Status::Playing).map(|s| format!("{}\n{}\n{}", s.id, s.title, s.artist));
        if announce(&mut self.last_song, now) && self.media_seen {
            self.show_hud(Hud::Track);
        }
        self.media_seen = true;
        self.update_ticker();
        let _ = self.render();
    }

    /// Redraw on a timer while music plays: waveform when closed, progress bar when open.
    pub(crate) unsafe fn update_ticker(&mut self) {
        let playing = self.current().is_some_and(|s| s.status == Status::Playing);
        // Paused music stays on the closed notch for Settings > Paused music's time, then the
        // clock comes back (the Music page stays, so it can be resumed).
        let paused = self.current().filter(|s| s.status == Status::Paused).map(|s| s.id.clone());
        match (&paused, &self.paused_since) {
            (Some(id), Some((since_id, _))) if id == since_id => {}
            (Some(id), _) => self.paused_since = Some((id.clone(), Instant::now())),
            (None, _) => self.paused_since = None,
        }
        let limit = settings::PAUSED_HIDE[self.prefs.paused_hide].1;
        let paused_for = self.paused_since.as_ref().map(|(_, at)| at.elapsed());
        let timed_out = paused_out(paused_for, limit);
        match (paused_for, limit) {
            (Some(d), Some(secs)) if !timed_out => {
                let left = Duration::from_secs(secs as u64).saturating_sub(d).as_millis() as u32 + 50;
                SetTimer(Some(self.hwnd), PAUSED_TIMER, left, None);
            }
            _ => {
                let _ = KillTimer(Some(self.hwnd), PAUSED_TIMER);
            }
        }
        // The closed notch cross-fades between the clock and now playing.
        let loaded = self.current().is_some_and(|s| s.status != Status::Stopped) && !timed_out;
        let mix = if loaded { 1.0 } else { 0.0 };
        // Music playing or paused: never the thin bar (re-checked when that changes).
        if loaded != self.music {
            self.music = loaded;
            if self.covered && !self.hovered {
                self.set_bar_mode(true);
                self.set_expanded(false); // music set first, so this doesn't loop back here
            }
        }
        if self.music_mix.target != mix {
            self.music_mix.set_target(mix, spring::ROW);
            self.start_animation();
        }
        // ~30 fps while music plays: the waveform is visible closed and on every open page
        // (the progress bar rides along).
        let ms = if playing { 33 } else { 0 };
        if ms != self.tick_ms {
            self.tick_ms = ms;
            if ms == 0 {
                let _ = KillTimer(Some(self.hwnd), TICK_TIMER);
            } else {
                SetTimer(Some(self.hwnd), TICK_TIMER, ms, None);
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::{Paused, Playing, Stopped};

    // R1.04: a new song that starts playing gets the pop-up; the same song resuming, or a
    // paused song, doesn't.
    #[test]
    fn new_song_pop_up_only_for_a_new_playing_song() {
        let mut last = None;
        assert!(announce(&mut last, Some("a".into())), "a song starts playing");
        assert!(!announce(&mut last, Some("a".into())), "same song still playing");
        assert!(!announce(&mut last, None), "paused or nothing loaded: no pop-up");
        assert!(!announce(&mut last, Some("a".into())), "the same song resumes: no pop-up");
        assert!(announce(&mut last, Some("b".into())), "next song");
    }

    // R1.04: Settings > Paused music: back to the clock after a while.
    #[test]
    fn paused_music_times_out() {
        use std::time::Duration;
        let secs = Duration::from_secs;
        assert!(!paused_out(None, Some(30)), "not paused");
        assert!(!paused_out(Some(secs(29)), Some(30)));
        assert!(paused_out(Some(secs(30)), Some(30)), "paused for 30 s: back to the clock");
        assert!(!paused_out(Some(secs(3_600)), None), "Never: stays");
    }

    #[test]
    fn track_times_format() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(3725.0), "1:02:05");
        assert_eq!(format_time(-3.0), "0:00");
        assert_eq!(format_time(f64::NAN), "0:00");
    }

    // F2: the media source you picked stays put.
    #[test]
    fn source_stays_when_another_app_starts_playing() {
        let s = [("spotify", Paused), ("chrome", Playing)];
        assert_eq!(pick_source(Some("spotify"), &s, Some("chrome")).as_deref(), Some("spotify"));
    }

    #[test]
    fn source_ignores_windows_current_changing() {
        let s = [("spotify", Playing), ("chrome", Playing)];
        assert_eq!(pick_source(Some("spotify"), &s, Some("chrome")).as_deref(), Some("spotify"));
    }

    #[test]
    fn second_browser_session_can_stay_selected() {
        let s = [("chrome", Playing), ("chrome#1", Paused)];
        assert_eq!(pick_source(Some("chrome#1"), &s, Some("chrome")).as_deref(), Some("chrome#1"));
    }

    #[test]
    fn source_gone_falls_back_to_playing_windows_current_first() {
        let s = [("vlc", Playing), ("chrome", Playing)];
        assert_eq!(pick_source(Some("spotify"), &s, Some("chrome")).as_deref(), Some("chrome"));
        assert_eq!(pick_source(Some("spotify"), &s, None).as_deref(), Some("vlc"));
    }

    #[test]
    fn stopped_source_counts_as_gone() {
        let s = [("spotify", Stopped), ("chrome", Paused)];
        assert_eq!(pick_source(Some("spotify"), &s, None).as_deref(), Some("chrome"));
    }

    #[test]
    fn nothing_selected_prefers_playing_then_paused() {
        let s = [("spotify", Paused), ("chrome", Paused), ("vlc", Playing)];
        assert_eq!(pick_source(None, &s, Some("chrome")).as_deref(), Some("vlc"));
        let s = [("spotify", Paused), ("chrome", Paused)];
        assert_eq!(pick_source(None, &s, Some("chrome")).as_deref(), Some("chrome"));
        assert_eq!(pick_source(None, &s, None).as_deref(), Some("spotify"));
    }

    #[test]
    fn all_stopped_shows_nothing() {
        assert_eq!(pick_source(Some("spotify"), &[("spotify", Stopped)], Some("spotify")), None);
        assert_eq!(pick_source(None, &[], None), None);
    }

}
