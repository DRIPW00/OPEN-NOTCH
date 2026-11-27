# Changelog

## R1.03: renamed to OpenNotch

### Changed
- **The app is now called OpenNotch.** The exe is `release\OpenNotch.exe`, and the window, tray menu and Settings say OpenNotch. Your settings move over by themselves (copied once from `%APPDATA%\DynamicIsland` to `%APPDATA%\OpenNotch`), and if "Launch at startup" was on it stays on under the new name.

## R1.03

### Fixed
- Album art could stay blank for the first song after opening Apple Music, until you skipped tracks. The notch now reloads the cover when the app signals a change, and for the first 30 seconds of each song it also notices when the app quietly swaps a placeholder for the real cover.

### Under the hood
- More of the code is split into smaller files (window, home); no change in behaviour.
- 71 automatic tests.

## R1.02

### New
- **Now playing on the closed notch.** While music plays, the closed notch shows the album art on the left and a moving waveform in the album's colour on the right, like the iPhone. Paused music stays on the notch with the waveform resting flat; the clock only comes back once nothing is loaded.
- **Hidden in full screen.** When a game, video or anything else goes full screen, the notch disappears completely and frees its drawing memory, then comes back when you leave full screen. Maximised windows still get the thin bar.

### Changed
- **Music keeps the notch out of the thin bar.** While music is playing or paused, a maximised window no longer shrinks the notch to the bar. It goes back to the bar once no music app is loaded.
- **One waveform everywhere.** The old three-bar equalizer on the Home and Music pages is replaced by the six-bar waveform from the closed notch.
- **Wider shadow.** About twice as wide, and it runs all the way up the curves that meet the top of the screen. Near the top its downward shift eases to nothing, so it follows those curves evenly instead of pooling past their tips.
- **The thin bar** is now only for maximised apps; full screen hides the notch instead. Always show keeps it full size in both cases.

### Fixed
- Desktop AMD processors showed as "Core" on the System page; they now show e.g. "Ryzen 5 5600X".
- The hover delay box could round a delay like 0.855 s to "0.85" and change it when saved.
- Music apps with "_" in their exe name showed a cut-off name on the Music page.
- The camera/mic bubble could show a closed app's name for a new app.

### Under the hood
- The code is split into smaller files (shadow, waveform, pages, settings card); no change in behaviour.
- 69 automatic tests.
## R1.01

### New
- **Settings in tabs.** Settings (the gear on Home) is now split into General, Motion, Pages and About.
- **Launch at startup** (General): starts with Windows. It shows the real state, including when you turned it off in Task Manager, and keeps working if you move the folder.
- **Always show** (General): the notch stays full size over full-screen and maximised apps instead of shrinking to the thin bar.
- **Animation speed** (Motion), seven speeds: Slow, Normal, Fast, Faster, Gotta go faster, Faster faster fasterrrr, Wally West.
- **Hover delay** (Motion): type how long the pointer rests on the notch before it opens, from 0.15 to 1.5 seconds. The thin bar over full-screen apps uses it too.
- **Open on** (Motion): Hover, Click or Both. With Click or Both, the thin bar can be clicked as well.
- **Opens where you left off.** The notch opens on the page you were on last, even after a restart, or on Home if that page is gone.
- **Pages loop.** Next from the last page slides forward onto Home, and Previous from Home slides back onto the last page. It no longer rewinds through every page.
- **App icon.** The exe and the tray now have their own icon.

### Changed
- **Music sources stay put.** The app you pick on the Music page stays shown until it stops or closes, so another app starting to play no longer takes over. Two sessions from the same app are labelled apart, e.g. "Chrome" and "Chrome 2". Your keyboard's media keys still control the app Windows considers current, which may not be the one shown.
- **Shuffle and Repeat** only appear for apps that support them, so there are no more buttons that do nothing. The workaround that pressed the app's own buttons is gone.
- **The shadow is new:** thin, soft and close to the edge all the way round, including the curves at the top, with no bands or blocks.
- **The thin bar** opens after your hover delay instead of a fixed 0.75 s.
- **The hover delay** now defaults to 0.3 s; R1.00 opened instantly.

### Fixed
- Clicking a second app's tab on the Music page jumped to the Controls page.
- Connecting or disconnecting a Bluetooth audio device could also connect or disconnect another device with a similar name ("JBL Flip" and "JBL Flip 5").
- A 24-hour clock showed AM/PM on Home.
- A page listed twice as hidden in settings.ini couldn't be turned back on.
- Some photos on the file shelf showed as small icons instead of filling their tile.
- Media buttons could control the wrong session when one app had two.
- Wi-Fi could get stuck on "Joining…" if the adapter disappeared mid-join, and every Wi-Fi click was then ignored.
- A failed Wi-Fi join could delete a network you had already saved, along with its password.
- Song changes could show up to 4 seconds late after an app swapped one media session for another.
- The first CPU reading after opening the System page was out of date.

### Under the hood
- No new dependencies. The exe is 0.77 MB, including the icon.
- Removed unused code: debug-only printouts, about 110 lines of tray-icon drawing (it now uses the app icon), and 9 redundant Windows features in Cargo.toml.
- 43 automatic tests (8 in R1.00).
- Privacy: still no internet connections. The only new thing written outside the app's settings file is the Windows startup entry, and only while Launch at startup is on.
