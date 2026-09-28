# mentarch-recorder

A small native Rust application for Hyprland: **pick a window → record → stop → get a video**. No OBS, scene setup, or recording configuration required.

## Experience

- Open the app and press **Choose window** to use Hyprland’s existing portal picker.
- After selection, show the chosen target and a clear **Record** button.
- During recording, show elapsed time and a prominent **Stop & save** button.
- Save automatically under `~/Videos/Recordings/` with a timestamped filename; offer **Open video** and **Show folder**.
- Handle cancellation and errors without leaving capture sessions or background recording running.

## Implementation

1. **Prove window capture first.** Check installed portal support; build a minimal Rust capture prototype using the ScreenCast portal through `ashpd`. Request window sources, obtain the PipeWire stream and remote file descriptor, and feed them to GStreamer. Verify this records the actual window, not a screen rectangle.
2. **Build the recording engine.** Use Rust GStreamer bindings to encode the stream into an MKV file. Detect available encoders, prefer compatible hardware encoding, and provide a software fallback. Stop with EOS and wait for file finalization before reporting success. Keep the portal session and PipeWire descriptor alive throughout recording.
3. **Add a compact native UI.** Use GTK4 Rust bindings with explicit idle, selecting, ready, recording, saving, and error states. Keep encoding and portal operations asynchronous so the interface stays responsive. Start with video only and sensible defaults.
4. **Integrate with the desktop.** Add an application launcher and optional Waybar launch action. Avoid changing existing desktop configuration automatically. Document dependencies and local build/install commands.
5. **Test the complete flow.** Cover picker cancellation, repeated recordings, window closure, resizing, overlapping windows, workspace changes, app exit, encoder failures, and unwritable output directories. Check playback and whether hidden windows continue producing frames; document any limitations.

## Scope

The MVP records **one selected window**, not every window belonging to an application. Reuse the portal picker rather than building a custom one. No OBS dependency, microphone/application audio, webcam, editing, or replay buffer initially. Audio and a persistent bar recording indicator can follow once basic recording is reliable.

## Done when

Launching the app, selecting a window, recording, and saving a playable video works reliably on this machine without a terminal or manual capture setup.
