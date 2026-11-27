# R1.01 test log

- **Date:** 27 Nov 2026
- **Build:** `Opennotch\release\Dynamic Island.exe`, built with `build.bat` (Rust release, MSVC). 0 warnings, 777,728 bytes (includes the icon).
- **Privacy check:** the Windows user name appears 0 times in the exe.
- **Tested on:** Windows 11 laptop (Ryzen 7 7840HS, RTX 4060 Laptop GPU).

The tests ran in three layers:

1. **Automatic tests.** `cargo test`: 48 tests, all passing. 40 are new in R1.01; 8 were there before. The 3 Watch-style tests were removed along with the feature.
2. **Independent code review.** A separate reviewer read the whole R1.01 diff and found 5 issues. All 5 were fixed and retested.
3. **On-screen checks.** The built app was run on the PC and checked with real hovers, clicks, drags and a restart.

Left out of R1.01, by decision:

- Discord mute/deafen: Discord only allows it through an app each user registers with Discord.
- The pull-down bounce: built and tested, then removed on request (see Changes after the first build).

## 1. Automatic tests

| Feature | Test | Scenario | Result |
|---|---|---|---|
| Media source stays put | `source_stays_when_another_app_starts_playing` | Spotify is picked and paused; Chrome starts playing. Spotify stays. | Pass |
| | `source_ignores_windows_current_changing` | Windows' "current" app switches to Chrome. The picked app stays. | Pass |
| | `second_browser_session_can_stay_selected` | Two Chrome sessions; the second one is picked and stays. | Pass |
| | `source_gone_falls_back_to_playing_windows_current_first` | The picked app closes. The notch moves to the playing app, Windows' current one first. | Pass |
| | `stopped_source_counts_as_gone` | The picked app stops. The notch moves to an app that's still live. | Pass |
| | `nothing_selected_prefers_playing_then_paused` | Nothing picked: a playing app, then Windows' current paused app, then the first paused app. | Pass |
| | `all_stopped_shows_nothing` | Everything stopped or none: nothing is shown. | Pass |
| Last tab | `last_page_round_trips` | Network is saved and then loaded back. | Pass |
| | `last_page_defaults_to_home` | Missing, garbage or "home" all give Home. | Pass |
| | `old_settings_file_still_loads` | An R1.00 settings file loads, and new options get their defaults. | Pass |
| Launch at startup | `run_command_quotes_paths_with_spaces` | The path "…\Dynamic Island.exe" is quoted. | Pass |
| | `task_manager_disabled_counts_as_off` | Turned off in Task Manager (StartupApproved 03/07) shows as off; 02/06 or no data shows as on. | Pass |
| | `registry_text_drops_trailing_nul` | The registry string is read back without its terminator. | Pass |
| | `only_a_missing_exe_counts_as_stale` | A second copy doesn't take over startup; a moved exe is re-pointed. | Pass |
| Always hide | `always_hide_round_trips` | Saved and loaded; only "1" turns it on. | Pass |
| Animation speed and hover delay | `motion_choices_round_trip` | Faster and an 0.85 s delay are saved and loaded. | Pass |
| | `motion_choices_reject_bad_values` | `speed=9` and `hover_ms=-1` fall back to the defaults; 99999 and 5 are pulled into 0.15–1.5 s. | Pass |
| | `default_speed_and_delay` | Defaults are normal speed and a 0.3 s delay. | Pass |
| | `typed_delay_is_read_in_seconds_and_kept_in_range` | "0.4", ".75", "1" and " 0.5 s " are read. 0.01 becomes 0.15 and 9 becomes 1.5. "", ".", "abc" and "inf" keep the old value. | Pass |
| | `delay_shows_without_trailing_zeros` | The box shows "0.3", "0.75", "1.5" and "1", and what it shows reads back the same. | Pass |
| | `animation_speed_orders_settle_times` | Slower takes longer than Normal, which takes longer than Faster, for opening and closing; all finish within 2 s. | Pass |
| Watch-style status | `watch_shows_battery_on_laptops_gpu_on_desktops` | A laptop shows CPU, RAM and BAT; a desktop shows CPU, RAM and GPU. | Pass |
| | `watch_shows_nothing_measured_as_unknown` | Before the first reading the gauges show "–", not 0. | Pass |
| | `watch_status_is_optional_and_remembered` | Off by default; saved and loaded. | Pass |
| Old settings file | `removed_bounce_setting_is_ignored` | A settings file with the removed `bounce=` and `hover_delay=` lines still loads, and they're dropped on the next save. | Pass |
| Shadow | `shadow_fades_smoothly` | Closed, half-open and open: peak darkness is 20–45%. It only gets lighter outwards, never steps more than 3.5% per 0.1 px, and fades to nothing. | Pass |
| | `shadow_reaches_further_below_than_sideways_and_grows_open` | It reaches further below than to the sides, and grows as the card opens. | Pass |
| Existing (R1.00) | 8 tests: Bluetooth matching, SVG icons, artist cleanup, settings parsing, easing, CPU/GPU names, Wi-Fi profile escaping | No regressions. | Pass |

The old shadow (8 equal bands of 5–7% each) would fail the "no harsh step" check.

`cargo clippy --all-targets`: clean.

## 2. Independent review

All five issues below were fixed and retested.

| # | Issue found | Fix |
|---|---|---|
| R1 | Turning on Watch-style status left the gauges at "–" until the notch was reopened. | Stats are switched on and off when Watch or Settings changes. Confirmed on screen (C5). |
| R2 | A track change (Windows' brief "Changing" state) could drop the media source you picked for another app. | "Changing" now counts as playing and "Opened" as paused. |
| R3 | With Always hide on, the full-screen bar could still open the hidden card. | Bar mode is off while hidden. |
| R4 | With Always hide on and no tray icon (for example, the taskbar not ready at startup), there was no way back. | Always hide is only applied at launch when the tray icon exists. |
| R5 | Running any second copy would take over "Launch at startup". | It only re-points the startup entry when the saved exe no longer exists, and a test was added. |

## 3. On-screen checks

| # | Scenario | Result |
|---|---|---|
| C1 | First hover after install. | Opens on Home (no saved tab yet). Pass |
| C2 | Settings → General. | Three rows: Launch at startup, Watch-style status, Always hide. Pass |
| C3 | The General, Motion, Pages and About tabs. | Each tab shows and the card resizes. Motion defaults are Normal speed and Off hover delay. Pages has drag handles. About reads "Version R1.01 · Made by DRIPWOO". Pass |
| C4 | Launch at startup switch. | **Not run.** The safety check stopped me from adding a Windows startup entry on your PC. Flip it yourself, then check Task Manager → Startup apps. |
| C5 | Turn on Watch, then Done. | Gauges show live values straight away (CPU 20, RAM 65, BAT 100): open 270° arcs, label in the gap. Pass |
| C6 | Go to System, move away, hover again. | Reopens on System. Pass |
| C9 | Always hide on. | The notch and shadow vanish, and hovering the top edge opens nothing. Pass |
| C10 | Tray icon → "Hide notch" (ticked) → click. | The notch comes back. The item is unticked next time. Pass |
| C11 | Hover delay Long, hover again (first build, with the 4 choices). | Still closed just after the pointer arrives; open at 1.2 s. Pass. **This missed the full-screen bar; see "Third round".** |
| C12 | Leave it on Controls, quit from the tray, relaunch the exe. | Reopens on Controls. Watch on and Long hover delay both kept. Pass |
| C13 | Shadow, open and closed, close-up. | Soft, darkest at the card's edge, no bands or trails. Pass (dark desktop only) |
| C14 | Settings reset to defaults afterwards. | Home shows the battery ring again. Pass |

## Changes after the first build

| What | Detail | Check |
|---|---|---|
| Bug: clicking a second source tab on the Music page jumped to Controls | Found by you, and older than R1.01. The tab click briefly emptied the list of media sessions while re-picking, so for that moment the Music page didn't exist and the notch "stayed" on the page that took its place (Controls). Fixed by copying the list instead of emptying it. | Compiles; source-picking tests pass. **Not checked on screen:** no media was loaded when I tried. |
| Pull-down bounce removed | The gesture, the Motion setting, the saved value and the 4 tests are gone. Press, release and mouse-capture handling are back to exactly R1.00. | 34 of 34 tests pass, clippy clean, Windows build has no warnings. |

## Third round: hover delay, clean-up and icon

**Hover delay "doesn't work".**

- **Cause:** whenever a maximised or full-screen app is in front, the notch is the thin bar. The bar opened after a fixed 0.75 s and never read the setting. My first check (C11) passed only because the window in front wasn't maximised.
- **Fix:** one delay now applies to both the notch and the bar.
- **Setting:** the four choices are replaced by a box where you type 0.15–1.5 seconds. It reuses the Wi-Fi password box's keyboard handling.

**Clean-up.** The compiler reports every unused function, constant, field and enum variant, including `pub` ones (checked with a deliberately unused probe), and it reported none. Removed:

- The four debug-only printouts and the two fields that only fed them. Release builds never ran them.
- The tray icon's ~110 lines of runtime drawing; the tray now loads the exe's icon.
- 9 Windows features that `Cargo.toml` listed although other features already include them.
- Stale comments.

**Icon.**

- `assets/icon.ico` holds the full tile from 32 px up, and the tray's pill design at 16–24 px.
- `build.rs` turns it into a Windows resource file with no extra tools or crates. Its output is byte-for-byte identical to LLVM's resource compiler (`llvm-rc`).

| # | Scenario | Result |
|---|---|---|
| D1 | Motion tab. | The box shows the default 0.3 with an "s" after it, and the hint "0.15 – 1.5 seconds". Pass |
| D2 | Click the box, type 1.2, press Enter. | Blue editing outline, typed text shows, 1.2 is kept. Pass |
| D3 | Type 5, press Enter. | It becomes 1.5. Pass |
| D4 | Type 1.2, then click another tab instead of pressing Enter. | It's kept as 1.2. Pass |
| D5 | Normal notch, 1.2 s delay. | Closed at 0.5 s, open at 1.5 s. Pass |
| D6 | Maximised window (thin bar), 1.2 s delay. | Still the bar at 1.0 s (the old code opened at 0.75 s), open at 1.6 s. Pass |
| D7 | The exe in Explorer. | The pill at small size, the tile at extra-large size. Pass |
| D8 | Tray icon. | The new pill with the green dot, loaded from the exe. Pass |
| D9 | Settings afterwards. | The delay is set back to 0.3, and Explorer back to its size and Details view. Pass |

## Fourth round: shadow shader, Always show, page loop, speeds, Open on

**Shadow.** Rewritten as a per-pixel shader running on the CPU. Chosen over wgpu, which would have added about 5–10 MB to the 0.75 MB exe and a GPU→CPU copy every frame.

- For each pixel in the thin band around the notch, it takes the exact signed distance to the outline (round corners included).
- It applies a Gaussian falloff with a blur width of 1.6–2.8 px, so the shadow is gone within about 3.5× that width of the edge.
- Each pixel gets a fixed random offset under one 8-bit step before rounding (dithering), so the fade has no bands.
- Pixels are written straight into the shadow window's bitmap, so there are no stacked layers and no blocks.

| Test | Scenario | Result |
|---|---|---|
| `outline_distance_is_exact` | Distance to the bottom edge, side, inside and the round corner (along its diagonal) is exact. | Pass |
| `shadow_is_thin_smooth_and_hugs_the_edge` | It only gets lighter going out, never steps more than 0.02, is gone by its reach, and the reach is under 14 px. | Pass |
| `painted_shadow_has_no_banding` | Down the straight side, each column's average is within 0.3 of an 8-bit step of the smooth curve. With dithering switched off as a probe, this test **fails** (21 vs 21.37), so it really catches banding. | Pass |
| `painted_shadow_fits_the_window` | A notch wider than the window, and a zero-strength shadow, are both safe. | Pass |
| `shadow_leaves_the_flared_top_corners_alone` | Code-review finding, fixed: nothing under the curve of each top corner, full shadow down the side. | Pass |

**Always show** replaces Always hide; the tray icon's Hide notch item is removed. With Always show on, the notch never shrinks to the thin bar. Test: `always_show_round_trips`, which also checks a leftover `always_hide=1` is ignored.

**Page loop.** The pages sit on a ring: Next from the last page slides forward onto Home, and Previous from Home slides backward onto the last page.

| Test | Scenario | Result |
|---|---|---|
| `ring_offset_wraps_the_short_way` | Each page is placed the short way round the ring. | Pass |
| `last_to_home_slides_forward_smoothly` | Home comes in from the right and the last page leaves left, each moving one way only, with no jumps. | Pass |
| `home_to_last_slides_backward_smoothly` | The reverse direction, the same checks. | Pass |
| `two_pages_still_slide` | Looping works with only two pages. | Pass |

**Seven speeds:** Slow, Normal, Fast, Faster, Gotta go faster, Faster faster fasterrrr, Wally West (0.7× to 10×).

| Test | Scenario | Result |
|---|---|---|
| `animation_speed_orders_settle_times` | Each speed is quicker than the one before. Slow settles in under 2 s; Wally West in 8 frames or fewer. | Pass |
| `wally_west_stays_stable` | At 10× every spring and slide still lands exactly. | Pass |

**Open on: Hover / Click / Both.** In Click and Both, the full-screen bar takes clicks instead of passing them through. Test: `open_modes_decide_hover_and_click`.

**On screen:**

| # | Scenario | Result |
|---|---|---|
| E1 | Shadow close-up, closed and open. | Thin, right at the edge; no blocks or bands. Pass |
| E2 | Next from System (the last page). | Mid-slide, System leaves left and Home comes in from the right, then lands on Home. Pass |
| E3 | Previous from Home. | Home leaves right and System comes in from the left. Pass |
| E4 | Speed stepper. | Every name fits ("Faster faster fasterrrr" too). Stops at Wally West with › dimmed, then stepped back to Normal. Pass |
| E5 | Open on: Click. | Hovering 1.5 s: still closed. Clicking: opens. Pass |
| E6 | Open on: Click, maximised window (thin bar). | Clicking the bar opens it. Pass |
| E7 | Always show on, maximised window. | The notch stays full size instead of the bar. Pass |
| E8 | Settings afterwards. | Always show off, Open on Hover, speed Normal, Explorer restored. Your own hover delay (0.15) and Launch at startup (on) were left as you set them. Pass |

## Fifth round: shadow rework, Watch-style status removed

**How the shadow was checked.** The real shader code, taken straight from `main.rs`, was run in a small offline program at your 125% scale. Its output was composited the way Windows does it (background, then shadow, then the black notch with its flared top corners) over light, mid-grey and dark backgrounds, then inspected at real size, 2× and 8×.

| Pass | Seen | Change |
|---|---|---|
| 1 | The fade added in the code review made the side shadow start abruptly, partway down the side, just below each top corner's flare, and the curve itself got no shadow. | The flare's real curve was added to the distance field, and the shadow fades in smoothly towards the screen edge. |
| 2 | A faint grey crease ran sideways from each flare: combining distance fields with max() underestimates distances out there. | Exact distance in each region: to the curve when facing it, to the flare's tip beyond it, to the body elsewhere. |
| 3 | Clean at 8×, real size and 2×, on every background: a thin, soft contact shadow hugging the edge all the way round, with no bands, blocks or crease. Invisible on dark backgrounds, as a real shadow would be. | Made a little thinner: blur width 1.4–2.2 px (was 1.6–2.8). |

| Test | Scenario | Result |
|---|---|---|
| `flare_distance_is_exact` | The distance facing the flare's curve, inside the flare, beyond its tip and below it on the straight side is exact. With pass 2's max() version as a probe, it **fails**. | Pass |
| `shadow_wraps_the_flare_smoothly` | Nothing on the screen-edge row. Down the side, through the flare, no jump over 12 of 255 between rows, full shadow below. Out sideways at the flare's height it only fades. With pass 1's fade as a probe, it **fails**. | Pass |
| `removed_bounce_setting_is_ignored` | Also checks that a leftover `watch=` line is dropped. | Pass |
| All earlier shadow tests | Distance, thin and smooth, no banding, fits the window. | Pass |

**Watch-style status removed:** the setting, its Settings row, the Home gauges, the stats sampling on Home, and the gauge drawing helper. `gfx.rs` and the Home drawing code are byte-for-byte back to R1.00.

| # | Scenario | Result |
|---|---|---|
| F1 | Settings › General. | Only Launch at startup and Always show. Pass |
| F2 | Home. | Battery ring as before. Pass |

## Sixth round: hidden in full screen, crisp shadow edges, now playing on the closed notch

**Hidden in full screen.** The window in front is now classified as maximised (taskbar showing: thin bar, as before) or full screen (covers the whole monitor: game, video, F11). In full screen:

- The notch and its shadow are hidden.
- Their two screen-sized pixel buffers are swapped for 1×1 ones (about 4 MB freed at 125% scale).
- Drawing stops and the full-screen bar's polling is off.
- Everything comes back when full screen ends. Always show keeps it visible.

| Test | Scenario | Result |
|---|---|---|
| `full_screen_and_maximised_are_told_apart` | Full screen: exact, a few pixels beyond the edges (F11), borderless "maximised", and on a second monitor. Maximised: taskbar showing, or an auto-hiding taskbar. Plus an ordinary window. | Pass |

**Shadow edges.** On red (like your reference) at 5×, the darkest shadow sat right against the edge and blurred the rounded corners.

- **Shadow shape:** it's now the notch's shape blurred, the same maths as CSS `box-shadow` (a complementary error function, `erfc`). That puts it at half strength at the edge, darker underneath and lighter outside, so the corners stay crisp.
- **Bug found in the renders:** with the shadow shifted down more than 2 px, a bright line appeared under the bottom edge, because rim pixels were skipped. Fixed.

| Test | Scenario | Result |
|---|---|---|
| `notch_edge_stays_crisp` | Half strength exactly at the edge, symmetric about it, full underneath, gone outside. | Pass |
| `no_light_gap_under_the_bottom_edge` | Every pixel on the bottom rim is shaded, for downward shifts of 0, 1, 2.75 and 4 px. With the old threshold as a probe, it **fails**. | Pass |
| `erfc_is_accurate` | Matches reference values to 1e-5. | Pass |
| Earlier shadow tests | Distance, flare, banding, fits. One expectation updated for the new shape. | Pass |

**Now playing on the closed notch.** While music plays, the clock cross-fades to album art on the left and a 6-bar waveform in the album's colour on the right, like your iPhone screenshot. The camera/mic dots stay; pausing fades back to the clock.

| Test | Scenario | Result |
|---|---|---|
| `waveform_moves_while_playing_and_rests_when_paused` | Every bar stays in range and visibly moves, never jumps more than a third of its height per frame at 30 fps, and sits low and still when paused. The middle bars rise higher than the ends. | Pass |

| # | Scenario | Result |
|---|---|---|
| G1 | Build on Windows. | No warnings, no user name in the exe. Pass |
| G2 | Open card over a maximised Explorer (bar mode, Open on: Hover). | Opens from the bar as before. Pass |
| G3 | Now-playing view and full-screen hiding. | **Not run.** Nothing was playing, and putting an app in full screen needs a key press (Explorer only allows clicks). |

## Seventh round: music and the thin bar, one waveform, wider shadow

**Changes.** Playing music keeps the notch out of the thin bar. Paused music stays on the closed notch. The open card's equalizer became the same six-bar waveform. The shadow is about twice as wide and has no top fade, so the flared corners that meet the screen edge are shadowed too.

| Test | Scenario | Result |
|---|---|---|
| `every_edge_has_shadow` | Every pixel 0.25–1.25 px outside the outline (sides, bottom corners, flares up to the screen edge; 686 pixels) gets at least a quarter of full strength. | Pass |
| `shadow_wraps_the_flare_smoothly` | Updated: the flare's tip on the screen-edge row now has shadow (it was asserted bare before). No jumps down the side, no crease. | Pass |
| `shadow_is_thin_smooth_and_hugs_the_edge` | Updated for the wider sigmas (3 and 4.5 px): smooth, gone within 18 px. | Pass |
| Other shadow tests, `erfc_is_accurate`, waveform test | Unchanged. | Pass |

These 11 ran as a standalone test build of the extracted shader and waveform code. The full `cargo test` needs Windows and wasn't run this round. The bar and paused logic has no unit test; it was checked on screen instead:

| # | Scenario | Result |
|---|---|---|
| H1 | Build on Windows. | No warnings. Pass |
| H2 | Apple Music playing, Explorer maximised. | Full notch with album art and waveform, not the bar. Pass |
| H3 | Open card, Home and Music pages. | New waveform beside the battery and above the progress bar, no overlap. Pass |
| H4 | Pause from the notch. | Notch drops to the bar (maximised window); open card shows the waveform resting flat. Resume brings the full notch back. Pass |
| H5 | Paused with no maximised window. | **Not run** (would mean un-maximising your Explorer). |

## Eighth round: paused music and the flare shadow

**Changes.** Paused music now also keeps the notch out of the thin bar. You reported the shadow on the flared top corners looked jagged and disconnected. The cause: the shadow is shifted down a couple of pixels, so near the screen edge it pulled away from the curve and pooled into a lump past the flare's tip. Measured 1 px outside the curve, it rose to 1.6× the straight side's shade (open notch) and then dropped sharply at the tip. The shift now eases to zero over the top of the notch, and the curve measures within 0.8–1.15× the side.

| Test | Scenario | Result |
|---|---|---|
| `flare_shadow_is_even_up_to_the_screen_edge` | Shade just outside the flare, every 5° from the side up to the screen edge, stays within 0.8–1.6× the straight side. With the old shader as a probe it **fails** (2.3×). | Pass |
| `shadow_wraps_the_flare_smoothly` | Column check starts one row lower (row 2 is now under the flare, since the shift is gone at the top). | Pass |
| The other 10 shadow and waveform tests | Unchanged. | Pass |

These 12 ran in the same standalone test build as the seventh round.

| # | Scenario | Result |
|---|---|---|
| I1 | Build on Windows. | No warnings. Pass |
| I2 | Apple Music paused, Explorer maximised. | Full notch with album art and the flat waveform, not the bar. Pass |
| I3 | The flare shadow on your screen. | **Not run.** Your screen was dark there; judged on the offline renders instead. |

## Not checked on screen

These need media playing or a light background, and I didn't start apps on your PC:

- **Hiding in full screen** (sixth round). Now playing was checked in the seventh round.
- **Shuffle/Repeat:** they should be hidden for browsers and shown for apps that report support.
- **Music page with two apps or two browser tabs:** the tab you pick should stay put, and labels should read "Chrome" / "Chrome 2".
- **Media keys:** they now control Windows' current app, which can differ from the app the notch shows.
- **Animation speed Slower/Faster:** covered only by the automatic test.
- **The shadow over a light app on your screen:** a screenshot can't show a 2–3 px gradient sharply enough, so it was judged on the offline renders of the real code (the fifth round) instead.
