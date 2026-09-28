# Validation

Automated checks do not establish compositor behavior. The portal picker and desktop-dependent cases below require a live Hyprland session.

## Automated checks

- Output reservation: same-timestamp recordings have unique names; existing files cannot be overwritten.
- Output failure: unwritable/invalid directories and empty output are rejected; unsuccessful publication preserves recoverable data.
- Canvas selection: odd dimensions, very small windows, HiDPI sizes, and invalid portal dimensions.
- GStreamer integration: encode a synthetic video, finalize it, then demux and decode the resulting file through EOS. Repeat with an odd-sized live source that resizes, followed by explicit Stop. No `.part` remains after successful finalization.
- Encoder fallback: a registered candidate that cannot negotiate is rejected, and the next working encoder is used.
- Corrupt container: a missing or zeroed EBML header cannot be published as a successful recording.
- Shared controls: selecting from the bar starts capture automatically; choosing from the window waits for Record. Initialization cannot reset a pending start. Duplicate clicks, late start events, cancellation, and quitting cannot accidentally start a second recording.
- Waybar output: elapsed time, state classes, and escaped tooltip text.

## Live session checklist

| Scenario | Expected result |
| --- | --- |
| Choose window → Record → Stop & save | A playable MKV; elapsed time advances; Open video and Show folder work |
| Cancel in portal picker | Return to idle; no capture or recording remains |
| Exit app while picker is open | App asks you to close the system dialog, then cleans up and exits; XDPH 1.4.1 cannot dismiss its synchronous picker programmatically |
| Cancel after selecting | Selected session closes; no file is created |
| Record repeatedly | New session each time, distinct files, no leftover streams |
| Close selected window before recording | Ready state ends with an actionable message |
| Close selected window while recording | Captured frames save if the encoder can finish; errors retain partial file |
| Resize selected window, including odd sizes | Stable output dimensions, correct aspect ratio, playable result |
| Place another window over the selected window | Video still contains only the selected window |
| Move target to another workspace / hide / minimize | Inspect whether frames update, freeze, or stop; document compositor behavior |
| Close recorder while ready | Controls hide; session remains ready for the bar's Record action |
| Close recorder during recording | Controls hide; recording and bar timer continue |
| Ctrl+Q / Save & quit during recording | Encoder finishes, file saves, recorder exits |
| Bar camera → select → timer → click again | Automatically records selection, then saves; no recorder window needed |
| Reload Waybar during recording | Same recorder PID and recording continue; timer reappears |
| Bar right-click menu | All commands work; duplicate start/stop commands are harmless |
| Unwritable `~/Videos/Recordings/` | Actionable error; no background recording remains |
| Broken hardware encoder | Encoder probe falls back to a working software encoder |
| Downstream encoder or storage failure | No false saved state; error identifies retained partial output |

Do not change permissions on existing personal recordings for testing. Use isolated temporary directories for failure tests.

## Machine validation record

2026-09-28, Arch Linux, Hyprland 0.56.2, XDPH 1.4.1, PipeWire 1.6.9, GStreamer 1.28.7, GTK 4.22.5:

- All 10 default/unit/GStreamer tests passed (`cargo test -- --include-ignored`); the separate live portal regression passed too. Formatting, desktop-entry validation, shell syntax validation, and Clippy across all targets/features passed.
- The final release binary and launcher were installed locally. The installed binary passed `--check` and opened a native recorder window.
- `--check` confirmed portal window support and selected software H.264 (`x264enc`). No usable hardware encoder was advertised on this machine.
- Synthetic capture: H.264, 640 × 360, 30 fps, 3.03 seconds; decoded successfully.
- Real portal capture: the returned source type was **Window**; a 10.07-second file contained 302 decodable frames. Inspection of an extracted frame showed only the animated test window.
- Full GUI flow: the user selected the test window, recorded, stopped, and confirmed “Your video is ready” / “Saved successfully.” The resulting 20.6-second MKV contained 618 H.264 frames, and a complete FFmpeg decode succeeded.
- Multiple live recordings produced distinct files and no remaining `.part` files after success.
- Regression found and fixed: `pipewiresrc on-disconnect=eos` flushes downstream when PipeWire removes buffers during renegotiation, which erased previously written file data. The application leaves this property at its default, owns copied buffers, watches portal closure/bus errors, and checks the Matroska header before publishing.
- Portal limitation found: cancelling by dropping the pending request and sending only `Session.Close` leaves XDPH's synchronous picker open and can block the next session. The application instead uses native picker cancellation and defers exit until that dialog returns.
- Live cancellation regression passed: two consecutive native-picker cancellations returned to idle, and quitting during a third pending picker exited after that picker was dismissed. The test harness terminated only the three test picker processes to simulate native dialog dismissal; no picker windows remained.
- Hardware behavior, real window closure, occlusion, minimized/off-workspace updates, Open video/Show folder, and app exit during an actual recording remain manual checklist items. The attempt to automate live resize/workspace checks encountered this machine's newer Lua dispatcher syntax; those cases are not marked as passed. Synthetic live resize and explicit-stop decoding did pass.

## Theme and Waybar validation

2026-09-28, Waybar 0.15.0, Charcoal Lavender:

- All 14 unit/controller/GStreamer tests passed; Clippy across all targets/features passed.
- A hidden recorder started in a transient user service. A separate `--show` invocation opened its controls while preserving the same PID; `--quit` exited cleanly and published inactive status. GTK emitted no CSS warnings, and the recorder window was visually checked against the local palette.
- The installed GTK3 menu passed `gtk-builder-tool validate`; the complete Waybar stylesheet loaded through `Gtk.CssProvider` without errors.
- The installer was exercised with an isolated two-bar JSONC config: comments/trailing commas parse, existing settings remain, backups are created, and a second installation makes no changes.
- The local Waybar config and stylesheet were backed up before integration. Waybar reloaded in place and the camera widget appeared next to the existing right-side modules; its appearance was visually checked.
- `--waybar` works without a display. Isolated runtime-file checks confirmed that an expired heartbeat or missing recorder process cannot be reported as active recording.
- Live widget flow passed: the user clicked the camera and approved the test window; recording began automatically with the controls hidden. Reloading Waybar preserved PID 492640 and advanced the timer from 21 to 24 seconds without stalled capture. Executing the widget's same left-click command stopped and saved the recording. The bar returned the green saved state; the service logged no errors.
- The resulting `Recording_2026-09-28_17-53-36.mkv` was H.264, 832 × 1080, 661 frames, 22.03 seconds. A complete FFmpeg decode passed. The test target was closed after successful finalization.
