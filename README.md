# mentarch-recorder

A small GTK4 window recorder for Hyprland. Choose one window in the existing system portal picker, press **Record**, then **Stop & save**. Videos are saved automatically in `~/Videos/Recordings/`; **Open video** and **Show folder** use your desktop's default applications.

The MVP captures video only. It does not record audio, a whole application, or every window belonging to an application.

## Dependencies

Rust 1.92+, GTK 4.12+, GStreamer 1.22+, PipeWire, and a ScreenCast portal supporting individual windows are required. On Arch Linux:

```sh
sudo pacman -S --needed rust base-devel pkgconf gtk4 gstreamer \
  gst-plugin-pipewire gst-plugins-base gst-plugins-good gst-plugins-bad \
  gst-plugins-ugly xdg-desktop-portal xdg-desktop-portal-hyprland python
```

Run inside a Hyprland Wayland session with PipeWire and both portal services running. `gst-plugins-ugly` supplies the preferred software H.264 encoder; VP8 is also supported. Working VA-API or NVIDIA H.264 encoders are preferred when GStreamer exposes them. An actual encode probe checks each candidate before selection, with software fallback if a hardware probe fails.

## Build and install

```sh
cargo run -- --check
cargo run
sh scripts/install.sh
```

The install script builds the release binary, installs it in `~/.local/bin`, and adds **Window Recorder** to the application launcher. It installs an icon, a desktop entry, and the Waybar menu under `${XDG_DATA_HOME:-~/.local/share}`. The app matches the local **Charcoal Lavender** palette, with Inter text and a JetBrains Mono timer. Styling is scoped to the recorder.

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
| Right-click | Choose/record, stop/save, cancel selection, open video/folder, show controls, or save and quit |

The bar shows elapsed time while recording, yellow while choosing/saving, green after saving, and an error marker if recording fails. Hover for details. The recorder can stay hidden throughout a recording. Closing its window hides the controls; **Ctrl+Q** or **Save & quit recorder** finalizes the video and exits. A desktop notification announces a successful background save.

All control commands target the same application instance. Waybar runs actions in transient `systemd --user` services, so reloading Waybar does not terminate the recorder. The user service manager must have the Wayland session environment (normally provided by the desktop session). No permanent service or autostart is installed. One-second status polling reads an atomic file under `$XDG_RUNTIME_DIR/mentarch-recorder/`, without starting GTK or requesting capture; stale active status is reported as an error.

CLI controls are also available:

```sh
mentarch-recorder --toggle       # Choose & record, or stop & save
mentarch-recorder --start        # Choose & record
mentarch-recorder --stop         # Stop & save
mentarch-recorder --show         # Show controls
mentarch-recorder --quit         # Save and exit
mentarch-recorder --waybar       # Waybar JSON
mentarch-recorder --status       # Full JSON status
mentarch-recorder --help         # All commands
```

To remove the widget, remove `custom/recorder` from the module list, its include from the bar config, and the recorder import from `style.css`, then reload Waybar.

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
cargo run -- --capture-test             # Portal picker + ten-second real window capture
cargo test
cargo test -- --ignored                 # GStreamer encode/decode integration tests
cargo clippy --all-targets -- -D warnings
```

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

Implementation references: [ashpd ScreenCast API](https://docs.rs/ashpd/0.13.13/ashpd/desktop/screencast/index.html), [ScreenCast protocol](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html), [Hyprland portal documentation](https://wiki.hypr.land/Hypr-Ecosystem/xdg-desktop-portal-hyprland/).
