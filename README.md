# Dynamic Island for Windows

An iOS-style Dynamic Island for Windows 10 and 11, written in Rust. It sits at the top centre
of your screen as a MacBook-style notch and uses almost no CPU while idle.

## Download and run

1. Download this repository (Code → Download ZIP) and unzip it, or just download
   **`release/Dynamic Island.exe`**.
2. Double-click **`Dynamic Island.exe`**. Nothing to install.

The first time, Windows may show "Windows protected your PC" because the app isn't signed:
click **More info → Run anyway**.

To quit, click the notch icon in the tray (by the clock) → **Quit Dynamic Island**.

## What it does

Hover the notch to open it (or click it; see Settings); move away and it closes after a short
pause. Scroll on the open notch, or click its left/right edges, to switch pages: they go round
in a loop. It opens on the page you were on last.

- **Home**: time, date and a battery ring. A green dot while an app uses the camera, an orange
  dot for the microphone; hover the dots to see which apps.
- **Music**: album art, a live equalizer and full controls for whatever is playing (Apple Music,
  Spotify, browsers…). Shuffle and repeat show for apps that support them. Other apps playing
  at the same time appear as tabs; the one you pick stays until it stops.
- **File shelf**: drag files onto the notch to keep them; drag them back out onto any folder or
  app (always as a copy, the original stays put).
- **Controls**: volume and brightness sliders, the audio output and input devices, and mute.
  Volume and brightness keys show a small slider in the closed notch.
- **Network**: Wi-Fi and Bluetooth switches; pick a Wi-Fi network (type a new network's password
  right in the notch) or connect headphones and speakers.
- **System**: CPU, RAM, GPU and SSD usage.
- **Settings** (gear on Home):
  - General: launch at startup, Always show (stay full size over full-screen and maximised
    apps).
  - Motion: animation speed (Slow to Wally West), the hover delay (how long the pointer rests
    on the notch before it opens: type 0.15 to 1.5 seconds), and Open on: Hover, Click or Both.
  - Pages: turn pages on or off and drag them into your own order.
- **Pin** (top-right of the open notch): keeps it open.
- **Full-screen and maximised apps**: the notch shrinks to a thin bar (unless Always show is on).
  Rest the pointer on it for the hover delay, or click it if Open on is Click or Both. With
  Open on: Hover, clicks pass through it to the app.

## Privacy

- Everything stays on your PC. The app makes no internet connections and collects nothing.
- The only file it writes is `%APPDATA%\DynamicIsland\settings.ini` (your settings and the
  last page you were on). "Launch at startup" adds one entry to Windows' startup apps
  (`HKCU\...\CurrentVersion\Run`), removed when you turn it off.
- Wi-Fi passwords you type are handed to Windows (which stores them like the Wi-Fi menu does)
  and are not kept by the app.

## Build it yourself

Needs [Rust](https://rustup.rs) (and the Visual Studio C++ build tools that its installer offers).

From a terminal in this folder:

```powershell
cargo build --release   # builds target\release\dynamic-island.exe
cargo run --release     # builds it and starts it
```

## Project layout

```
release/Dynamic Island.exe   the ready-to-run app
CHANGELOG.md                 what's new in each version
TEST_LOG.md                  every test and on-screen check for R1.01
Cargo.toml, Cargo.lock       Rust project and exact dependency versions
build.rs                     puts assets/icon.ico into the exe (no extra tools)
assets/icon.ico              the app and tray icon
src/
  main.rs        the notch window, pages, drawing and mouse/keyboard handling
  spring.rs      animation curves (springs and eased slides)
  gfx.rs         drawing helpers: icons, text, images, rings
  tray.rs        the tray icon and its menu (Hide notch, Quit)
  settings.rs    your settings and last page (saved to settings.ini)
  startup.rs     "Launch at startup" (Windows' startup apps)
  sysinfo.rs     clock, date and battery
  privacy.rs     which apps use the camera / microphone
  media.rs       every app's now-playing info and controls (Windows media controls)
  audio.rs       volume, mute, audio devices
  brightness.rs  built-in display brightness
  wifi.rs        Wi-Fi networks: scan, join, passwords
  bluetooth.rs   paired Bluetooth devices; connects headphones/speakers
  radios.rs      Wi-Fi and Bluetooth on/off
  shelf.rs       the file shelf: drop target, drag out, thumbnails
  stats.rs       CPU, RAM, GPU and disk usage (only sampled while the System page is open)
```

While animating it draws one frame per refresh of the monitor the notch is on (60, 120, 144,
240 Hz…). When nothing moves it draws nothing. Only one copy runs at a time.
