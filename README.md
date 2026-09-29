# mentarch-recorder

A small native window recorder for Hyprland, built with Rust, GTK4, PipeWire, and GStreamer. Record one window from a desktop app or control the entire recording from Waybar.

- Select a window through the system sharing picker.
- Record silently, with a microphone, with desktop audio, or with both.
- Choose audio devices using live activity meters.
- Start, stop, and configure recording from a Waybar widget with an elapsed-time indicator.
- Save playable MKV files locally, with hardware encoding when available and a software fallback.
- Use a compact **Charcoal Lavender** interface that matches the included Waybar styling.

This is an early Linux desktop application, tested on Arch Linux with Hyprland. Other compositors and portal backends have not been validated. It records one window at a time; it does not provide video editing, webcam overlays, or a replay buffer.

## Privacy

The recorder has no built-in upload service, accounts, analytics, or telemetry. Capture and encoding run on your machine. Videos are saved in `~/Videos/Recordings/`, audio preferences in `${XDG_CONFIG_HOME:-~/.config}/mentarch-recorder/`, and temporary widget status in `$XDG_RUNTIME_DIR/mentarch-recorder/`. Building the app downloads Rust dependencies; normal recording needs no internet connection.

Window capture requires approval in the system sharing picker. Microphone and desktop audio are off on a fresh install, and your choices persist after you change them. Desktop audio captures **everything playing through the selected output**, not just the recorded application. Audio settings temporarily open device streams for live meters while that window is visible; they do not save or play the preview audio.

Recordings are ordinary, unencrypted local files governed by your filesystem permissions and any backup or sync tools you configure. This repository contains source code and test fixtures, not personal recordings, installed device preferences, or desktop configuration.

## Dependencies

Rust 1.92+, GTK 4.12+, GStreamer 1.22+, PipeWire, and a ScreenCast portal supporting individual windows are required. On Arch Linux:

```sh
sudo pacman -S --needed rust base-devel pkgconf gtk4 gstreamer \
  gst-plugin-pipewire gst-plugins-base gst-plugins-good gst-plugins-bad \
  gst-plugins-ugly xdg-desktop-portal xdg-desktop-portal-hyprland python \
  pipewire-pulse libpulse
```

Run inside a Hyprland Wayland session with PipeWire and both portal services running. Audio uses PipeWire's PulseAudio service, `pactl` for device discovery, and the GStreamer PulseAudio/Opus plugins. `gst-plugins-ugly` supplies the preferred software H.264 encoder; VP8 is also supported. Working VA-API or NVIDIA H.264 encoders are preferred when GStreamer exposes them. An actual encode probe checks each candidate before selection, with software fallback if a hardware probe fails.

## Build and install

```sh
git clone https://github.com/Mentioum/mentarch-recorder.git
cd mentarch-recorder
cargo run -- --check
sh scripts/install.sh
```

The install script builds the release binary, installs it in `~/.local/bin`, and adds **Window Recorder** to the application launcher. It installs an icon, a desktop entry, and the Waybar menu under `${XDG_DATA_HOME:-~/.local/share}`. You can also run the app from the checkout with `cargo run`. Inter and JetBrains Mono are the preferred UI fonts, with system fallbacks; a Nerd Font is recommended for Waybar icons. Styling is scoped to the recorder.

## Record your first window

1. Open **Window Recorder** from your application launcher.
2. Set the microphone and desktop audio checkboxes; leave both off for a silent video. Use the gear button to choose devices if needed.
3. Click **Choose window**, select the window in the system sharing dialog, and approve sharing.
4. Click **Record**, then **Stop & save** when finished.
5. Use **Open video** or **Show folder** after the app says your video is ready.

With the optional Waybar widget, clicking the camera starts this flow and recording begins as soon as sharing is approved.

## Waybar controls

Install the widget into your existing Waybar configuration, then reload the bar:

```sh
python3 scripts/install_waybar.py
pkill -USR2 -x waybar
```

The installer adds `custom/recorder` at the start of `modules-right`, includes its JSON configuration, and imports its stylesheet. Existing settings are preserved; rewritten files receive timestamped `.before-recorder-*` backups. JSONC comments are removed from the rewritten config but remain in the backup. Running the installer again does not duplicate the widget. Waybar must support custom module menus; this was validated with 0.15.0.

| Gesture | Action |
| --- | --- |
| Left-click camera | Choose a window in the system picker; recording starts after sharing is approved |
| Left-click red timer | Stop and save |
| Middle-click | Open the latest saved video |
| Right-click | Recording controls, microphone/desktop checkboxes, Audio settings, files, and quit |

The bar shows elapsed time while recording, yellow while choosing/saving, green after saving, and an error marker if recording fails. Hover for details. The recorder can stay hidden throughout a recording. Closing its window hides the controls; **Ctrl+Q** or **Save & quit recorder** finalizes the video and exits. A desktop notification announces a successful background save.

All control commands target the same application instance. Waybar runs actions in transient `systemd --user` services, so reloading Waybar does not terminate the recorder. Its static GTK3 menu is regenerated and the bar reloaded when audio checkboxes change, keeping menu state consistent with the app after restarts. The user service manager must have the Wayland session environment (normally provided by the desktop session). No permanent service or autostart is installed. One-second status polling reads an atomic file under `$XDG_RUNTIME_DIR/mentarch-recorder/`, without starting GTK or requesting capture; stale active status is reported as an error.

CLI controls are also available:

```sh
mentarch-recorder --toggle       # Choose & record, or stop & save
mentarch-recorder --start        # Choose & record
mentarch-recorder --stop         # Stop & save
mentarch-recorder --show         # Show controls
mentarch-recorder --settings     # Audio devices and live meters
mentarch-recorder --start-silent # Choose & record video only
mentarch-recorder --start-mic    # Choose & record with microphone
mentarch-recorder --start-desktop # Choose & record with desktop audio
mentarch-recorder --start-both   # Choose & record with both
mentarch-recorder --quit         # Save and exit
mentarch-recorder --waybar       # Waybar JSON
mentarch-recorder --status       # Full JSON status
mentarch-recorder --help         # All commands
```

To remove the widget, remove `custom/recorder` from the module list, its include from the bar config, and the recorder import from `style.css`, then reload Waybar.

## Audio options

Use the right-click menu's **Record microphone** and **Record desktop audio** checkboxes, or the same controls in the main window. Both off produces a silent video; both on mixes the sources into one stereo audio track. The bar adds microphone/speaker icons and identifies the current audio mode in its tooltip.

Open **Audio settings…** from the menu or the recorder's gear button. Every microphone and desktop output has a level meter; speak or play sound to identify it. Select a device explicitly, or choose **Follow system default**, then **Save settings**. The default is resolved when recording starts. Device lists can be refreshed after plugging in headphones or a USB mic. Red meters indicate levels near clipping; microphone gain can be adjusted in the desktop's audio settings.

Settings previews listen only while their window is open. They do not play audio, save audio, change the default device, or change system device volumes. Closing settings stops those capture streams. Saving or changing checkboxes affects the next recording; an active recording keeps its original audio choices.

Settings live in `${XDG_CONFIG_HOME:-~/.config}/mentarch-recorder/audio.json`. Explicit device choices use stable server names, not numeric IDs. A missing selected device produces an error instead of silently choosing another. Capture inputs are pinned to the resolved device; incoming buffers are checked against the actual audio route to reject substitutions.

Audio is Opus, 48 kHz stereo, in the same MKV as the video. Microphone and desktop audio share the video pipeline's clock. When both are enabled, each contributes at half level to provide mixing headroom. No echo cancellation is applied; headphones avoid recording speaker output again through the mic.

## Capture and file behavior

- Only `SourceType::Window` is requested, with multiple selection disabled. The returned stream must also explicitly identify itself as a window; monitor/region results are rejected.
- The portal owns the picker. Some backend versions show extra picker choices despite the request; choose **Window**. The portal does not expose a reliable window title, so the app displays “Selected window” and the reported dimensions.
- The portal session and its PipeWire descriptor stay alive until encoding finishes. Cancelling in the system picker, selecting again, errors, and normal app exit close the session.
- Use **Cancel in the system sharing dialog** while choosing a window. XDPH 1.4.1 runs that dialog synchronously and cannot reliably dismiss it through `Session.Close`. If you request **Quit** while the picker is open, the recorder asks you to close the sharing dialog and waits to clean up; it does not abandon a picker that would block subsequent portal requests. After selection, the recorder's own **Cancel** releases the session immediately.
- Encoding and portal requests run off the GTK thread. Stop and normal app exit drain the encoder using EOS and wait for the muxer before showing success.
- Recordings use Matroska (`.mkv`), up to 1920 × 1080 at 30 fps. The starting size determines an even-sized canvas, preserving aspect ratio with borders when the window resizes. Portal sizes may be logical pixels on scaled displays.
- Files use a local timestamp and collision suffix. A reserved `.mkv.part` becomes `.mkv` only after EOS, nonempty encoded output, a valid Matroska header, and a disk flush. Incomplete nonempty files remain available for recovery after errors. Existing recordings are never overwritten.
- Capture loss attempts to save video already received. A failed save stays visible even if you requested app exit. Force-killing the process or losing power cannot guarantee finalization.
- Hidden/minimized/off-workspace windows depend on compositor behavior. The app shows “Waiting for window frames…” after five seconds without new encoded frames; it does not claim that a hidden window continues updating. See [the validation checklist](docs/TESTING.md).

## Diagnostics and validation

```sh
cargo run -- --check                    # No picker; portal + real encoder probe
cargo run -- --self-test                # Synthetic recording in /tmp/mentarch-recorder-test
cargo run -- --self-test-audio          # Three synthetic audio/video recordings; no real mic
cargo run -- --capture-test             # Portal picker + ten-second real window capture
cargo test
cargo test -- --ignored                 # GStreamer encode/decode integration tests
cargo clippy --all-targets -- -D warnings
```

Live audio integration with disposable virtual devices (requires `ffmpeg`):

```sh
cargo build
python3 scripts/test_audio.py
```

This checks mic-only, desktop-only, mixed audio, timing, playback, and pinned routing without recording a physical microphone. For device troubleshooting, `cargo run -- --audio-device-test` records three seconds from your **configured real audio devices** against a synthetic video in `/tmp/mentarch-recorder-device-test`.

Optional live cancellation regression (opens three real pickers; cancel each in its dialog):

```sh
cargo test --features live-portal-tests live_portal_cancellation_repeat_and_shutdown -- --ignored
```

`cargo run --example capture_target` opens an animated, harmless test window for capture and resize checks.

The capture test saves under `~/Videos/Recordings/`. It needs you to choose a window in the portal picker. The synthetic test does not capture your desktop.

For portal issues:

```sh
systemctl --user status xdg-desktop-portal xdg-desktop-portal-hyprland pipewire
journalctl --user -u xdg-desktop-portal-hyprland -b
```

Implementation references: [ashpd ScreenCast API](https://docs.rs/ashpd/0.13.13/ashpd/desktop/screencast/index.html), [ScreenCast protocol](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html), [GStreamer PulseAudio source](https://gstreamer.freedesktop.org/documentation/pulseaudio/pulsesrc.html), [audio mixer](https://gstreamer.freedesktop.org/documentation/audiomixer/audiomixer.html), [level meters](https://gstreamer.freedesktop.org/documentation/level/index.html).

## License

[MIT](LICENSE).
