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

Hover the notch to open it; move away and it closes after a short pause. Scroll on the open
notch, or click its left/right edges, to switch pages. It always opens on Home.

- **Home**: time, date and a battery ring. A green dot while an app uses the camera, an orange
  dot for the microphone; hover the dots to see which apps.
- **Music**: album art, a live equalizer and full controls for whatever is playing (Apple Music,
  Spotify, browsers…). Other apps playing at the same time appear as tabs.
- **File shelf**: drag files onto the notch to keep them; drag them back out onto any folder or
  app (always as a copy, the original stays put).
- **Controls**: volume and brightness sliders, the audio output and input devices, and mute.
  Volume and brightness keys show a small slider in the closed notch.
- **Network**: Wi-Fi and Bluetooth switches; pick a Wi-Fi network (type a new network's password
  right in the notch) or connect headphones and speakers.
- **System**: CPU, RAM, GPU and SSD usage.
- **Settings** (gear on Home): turn pages on or off and drag them into your own order.
- **Pin** (top-right of the open notch): keeps it open.
- **Full-screen and maximised apps**: the notch shrinks to a thin bar that clicks pass through;
  rest the pointer on it for a moment to open it.

## Privacy

- Everything stays on your PC. The app makes no internet connections and collects nothing.
- The only file it writes is `%APPDATA%\DynamicIsland\settings.ini` (which pages show, and
  their order).
- Wi-Fi passwords you type are handed to Windows (which stores them like the Wi-Fi menu does)
  and are not kept by the app.

## Build it yourself

Needs [Rust](https://rustup.rs) (and the Visual Studio C++ build tools that its installer offers).

Double-click **`build.bat`**. It builds an optimised exe into `release\Dynamic Island.exe`,
closes any running copy and starts the new one. Or from a terminal:

```powershell
cargo run            # debug build: a console shows refresh rate and fps per animation
cargo run --release  # optimised build, no console
```

## Project layout

```
release/Dynamic Island.exe   the ready-to-run app
build.bat                    builds release/Dynamic Island.exe
Cargo.toml, Cargo.lock       Rust project and exact dependency versions
src/
  main.rs        the notch window, pages, drawing and mouse/keyboard handling
  spring.rs      animation curves (springs and eased slides)
  gfx.rs         drawing helpers: icons, text, images, rings
  tray.rs        the tray icon and its Quit menu
  settings.rs    which pages show and their order (saved to settings.ini)
  sysinfo.rs     clock, date and battery
  privacy.rs     which apps use the camera / microphone
  media.rs       every app's now-playing info and controls (Windows media controls)
  uia.rs         presses an app's own shuffle/repeat buttons when it doesn't support them otherwise
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
