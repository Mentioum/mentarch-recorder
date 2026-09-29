use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    audio,
    model::{Model, State, format_elapsed},
    output,
};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub state: State,
    pub elapsed_seconds: u64,
    pub stalled: bool,
    pub detail: String,
    pub target: String,
    pub saved: Option<PathBuf>,
    pub pid: Option<u32>,
    pub updated_at: u64,
    #[serde(default)]
    pub audio: audio::Settings,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn path() -> Result<PathBuf> {
    Ok(dirs::runtime_dir()
        .context("XDG_RUNTIME_DIR is unavailable")?
        .join("mentarch-recorder/status.json"))
}

pub fn publish(model: &Model, stopped: bool) -> Result<()> {
    let snapshot = Snapshot {
        state: if stopped { State::Idle } else { model.state },
        elapsed_seconds: model.elapsed.as_secs(),
        stalled: model.stalled,
        detail: model.detail.clone(),
        target: model.target.clone(),
        saved: model.saved.clone(),
        pid: (!stopped).then_some(std::process::id()),
        updated_at: now(),
        audio: if matches!(
            model.state,
            State::Starting | State::Recording | State::Saving
        ) {
            model.active_audio.clone()
        } else {
            model.audio.clone()
        },
    };
    let path = path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(&snapshot)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn read() -> Snapshot {
    let mut snapshot = path()
        .ok()
        .and_then(|p| fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok())
        .unwrap_or_default();
    let alive = snapshot
        .pid
        .is_some_and(|pid| PathBuf::from(format!("/proc/{pid}")).exists())
        && now().saturating_sub(snapshot.updated_at) <= 5;
    if !alive && snapshot.pid.is_some() {
        if matches!(
            snapshot.state,
            State::Recording | State::Starting | State::Saving
        ) {
            snapshot.state = State::Error;
            snapshot.detail =
                "Recorder stopped unexpectedly. Check the recordings folder for a partial video."
                    .into();
        } else {
            snapshot.state = State::Idle;
        }
        snapshot.pid = None;
    }
    snapshot
}

pub fn waybar(snapshot: &Snapshot) -> serde_json::Value {
    let elapsed = format_elapsed(Duration::from_secs(snapshot.elapsed_seconds));
    let (mut text, class, title) = match snapshot.state {
        State::Idle => (
            "".into(),
            "idle",
            "Window Recorder · Click to choose and record".into(),
        ),
        State::Selecting => (
            " …".into(),
            "selecting",
            "Choose a window, or Cancel, in the sharing dialog".into(),
        ),
        State::Ready => (
            " Ready".into(),
            "ready",
            "Window selected · Click to record".into(),
        ),
        State::Starting => (
            "● …".into(),
            "starting",
            "Starting recording · Click to stop".into(),
        ),
        State::Recording => (
            format!("● {elapsed}"),
            "recording",
            format!(
                "Recording {elapsed} · Click to stop & save{}",
                if snapshot.stalled {
                    "\nWaiting for window frames"
                } else {
                    ""
                }
            ),
        ),
        State::Saving => (" Saving".into(), "saving", "Finalizing video…".into()),
        State::Saved => (
            "".into(),
            "saved",
            format!(
                "Saved · {}",
                snapshot
                    .saved
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
        ),
        State::Error => (
            " !".into(),
            "error",
            format!("{}\nClick to try again", snapshot.detail),
        ),
    };
    let settings = if snapshot.pid.is_none() {
        audio::Settings::load().unwrap_or_default()
    } else {
        snapshot.audio.clone()
    };
    let tooltip = format!(
        "{title}\nAudio: {}\nRight-click: recording menu\nMiddle-click: open latest video",
        settings.summary()
    );
    if settings.microphone {
        text.push_str(" ");
    }
    if settings.desktop {
        text.push_str(" ");
    }
    serde_json::json!({"text":text, "class":class, "alt":class, "tooltip":gtk::glib::markup_escape_text(&tooltip).as_str()})
}

pub fn latest_video() -> Option<PathBuf> {
    fs::read_dir(output::recordings_dir().ok()?)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("Recording_")
                && e.path().extension().is_some_and(|ext| ext == "mkv")
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            if m.is_file() {
                Some((m.modified().ok()?, e.path()))
            } else {
                None
            }
        })
        .max_by_key(|(time, _)| *time)
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bar_status_shows_recording_and_escapes_user_text() {
        let recording = Snapshot {
            state: State::Recording,
            elapsed_seconds: 65,
            ..Snapshot::default()
        };
        assert_eq!(waybar(&recording)["text"], "● 01:05");
        assert_eq!(waybar(&recording)["class"], "recording");
        let error = Snapshot {
            state: State::Error,
            detail: "<bad>&\"".into(),
            ..Snapshot::default()
        };
        let tooltip = waybar(&error)["tooltip"].as_str().unwrap().to_owned();
        assert!(tooltip.contains("&lt;bad&gt;&amp;"));
        assert!(!tooltip.contains("<bad>"));
    }
}
