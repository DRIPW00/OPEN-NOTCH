# OpenNotch

A MacBook-style notch for Windows 10 and 11. It sits at the top centre of your screen and
shows what's playing, your devices and your system at a glance. It's light too: when nothing
moves it draws nothing, so it uses almost no CPU while idle.

## Get started

1. Download **`release/OpenNotch.exe`** from this repository.
2. Double-click it. There's nothing to install.

The first time, Windows may say "Windows protected your PC" because the app isn't signed.
Click **More info → Run anyway**.

To quit, click the notch icon in the tray (by the clock) and choose **Quit OpenNotch**.

## Using it

Hover over the notch to open it, or click it if you set that up in Settings. Move away and it
closes after a moment. To switch pages, scroll on the open notch or click its left or right
edge; the pages go round in a loop, and it opens on the page you used last.

- **Home**: the time, date and battery. A green dot shows when an app is using the camera, an
  orange dot for the microphone; hover over the dots to see which apps.
- **Music**: album art, a live waveform and full controls for whatever is playing (Apple Music,
  Spotify, browsers and more). Shuffle and repeat appear for apps that support them. If several
  apps are playing, each gets a tab. While music plays, the closed notch shows the album art and
  a small waveform.
- **File shelf**: drag files onto the notch to keep them handy, then drag them out onto any
  folder or app. Files are always copied; the originals stay where they are.
- **Controls**: volume and brightness sliders, mute, and your audio output and input devices.
  The volume and brightness keys show a small slider in the closed notch.
- **Network**: turn Wi-Fi and Bluetooth on or off, join a Wi-Fi network (type the password right
  in the notch) or connect headphones and speakers.
- **System**: CPU, RAM, GPU and disk usage.
- **Pin** (top right of the open notch): keeps it open.

**Settings** (the gear on Home):

- **General**: launch at startup, and Always show, which keeps the notch full size even over
  full-screen and maximised apps.
- **Motion**: animation speed, how long the pointer rests on the notch before it opens (0.15 to
  1.5 seconds), and whether it opens on hover, click or both.
- **Pages**: turn pages on or off and drag them into your own order.

**Full-screen apps** (games, videos, F11): the notch hides completely and comes back when you
leave full screen. **Maximised apps**: it shrinks to a thin bar that opens like the notch does.

## Privacy

- Everything stays on your PC. The app makes no internet connections and collects nothing.
- It saves only one file, `%APPDATA%\OpenNotch\settings.ini`, with your settings and the last
  page you were on. "Launch at startup" adds one entry to Windows' startup apps, removed when
  you turn it off.
- Wi-Fi passwords you type go to Windows, which stores them as the Wi-Fi menu does. The app
  doesn't keep them.

## What's new

See [CHANGELOG.md](CHANGELOG.md).
