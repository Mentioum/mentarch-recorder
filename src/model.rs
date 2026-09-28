use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::worker::{Command, Event};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Idle,
    Selecting,
    Ready,
    Starting,
    Recording,
    Saving,
    Saved,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Background,
    Toggle,
    Start,
    Choose,
    Record,
    Stop,
    Cancel,
    Open,
    Folder,
    Quit,
}

impl Action {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim_start_matches('-') {
            "show" => Self::Show,
            "background" => Self::Background,
            "toggle" => Self::Toggle,
            "start" => Self::Start,
            "choose" => Self::Choose,
            "record" => Self::Record,
            "stop" => Self::Stop,
            "cancel" => Self::Cancel,
            "open" => Self::Open,
            "folder" => Self::Folder,
            "quit" => Self::Quit,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::Background => "background",
            Self::Toggle => "toggle",
            Self::Start => "start",
            Self::Choose => "choose",
            Self::Record => "record",
            Self::Stop => "stop",
            Self::Cancel => "cancel",
            Self::Open => "open",
            Self::Folder => "folder",
            Self::Quit => "quit",
        }
    }
}

pub struct Model {
    pub state: State,
    pub detail: String,
    pub target: String,
    pub encoder: String,
    pub saved: Option<PathBuf>,
    pub closing: bool,
    pub worker_alive: bool,
    pub elapsed: Duration,
    pub stalled: bool,
    auto_record: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            state: State::Idle,
            detail: String::new(),
            target: String::new(),
            encoder: String::new(),
            saved: None,
            closing: false,
            worker_alive: true,
            elapsed: Duration::ZERO,
            stalled: false,
            auto_record: false,
        }
    }
}

impl Model {
    pub fn new(saved: Option<PathBuf>) -> Self {
        Self {
            saved,
            ..Self::default()
        }
    }

    pub fn request(&mut self, action: Action) -> Option<Command> {
        if self.closing || !self.worker_alive {
            return None;
        }
        match action {
            Action::Toggle => match self.state {
                State::Idle | State::Saved | State::Error => self.request(Action::Start),
                State::Ready => self.request(Action::Record),
                State::Recording | State::Starting => self.request(Action::Stop),
                _ => None,
            },
            Action::Start | Action::Choose
                if matches!(
                    self.state,
                    State::Idle | State::Saved | State::Error | State::Ready
                ) =>
            {
                self.state = State::Selecting;
                self.detail.clear();
                self.target.clear();
                self.elapsed = Duration::ZERO;
                self.stalled = false;
                self.auto_record = action == Action::Start;
                Some(Command::Choose)
            }
            Action::Record if self.state == State::Ready => {
                self.state = State::Starting;
                Some(Command::Record)
            }
            Action::Stop if matches!(self.state, State::Starting | State::Recording) => {
                self.state = State::Saving;
                Some(Command::Stop)
            }
            Action::Cancel if matches!(self.state, State::Selecting | State::Ready) => {
                self.auto_record = false;
                self.detail = "Cancel or close the system sharing dialog to finish.".into();
                Some(Command::Cancel)
            }
            Action::Quit => {
                self.auto_record = false;
                self.closing = true;
                if matches!(self.state, State::Starting | State::Recording) {
                    self.state = State::Saving;
                }
                Some(Command::Shutdown)
            }
            _ => None,
        }
    }

    pub fn event(&mut self, event: Event) -> Option<Command> {
        match event {
            Event::Initialized => {}
            Event::Idle => {
                self.state = State::Idle;
                self.detail.clear();
                self.auto_record = false;
                self.elapsed = Duration::ZERO;
            }
            Event::Selecting => self.state = State::Selecting,
            Event::FinishPicker => {
                self.detail = "Cancel or close the system sharing dialog to finish.".into()
            }
            Event::Ready(target) => {
                self.target = target;
                self.state = State::Ready;
                if self.auto_record && !self.closing {
                    self.auto_record = false;
                    return self.request(Action::Record);
                }
            }
            Event::Starting => {
                if self.state != State::Saving {
                    self.state = State::Starting;
                }
            }
            Event::Recording { encoder } => {
                if self.state != State::Saving {
                    self.state = State::Recording;
                }
                self.encoder = encoder;
            }
            Event::Tick { elapsed, stalled } => {
                self.elapsed = elapsed;
                self.stalled = stalled;
            }
            Event::Saving => self.state = State::Saving,
            Event::Saved { path, note } => {
                self.state = State::Saved;
                self.detail = format!(
                    "{}{}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    note.map(|n| format!("\n{n}")).unwrap_or_default()
                );
                self.saved = Some(path);
            }
            Event::Error(error) => {
                self.state = State::Error;
                self.detail = error;
                self.auto_record = false;
                self.closing = false;
            }
            Event::Shutdown => self.worker_alive = false,
        }
        None
    }
}

pub fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bar_toggle_selects_then_records_and_stops_once() {
        let mut model = Model::default();
        assert!(matches!(
            model.request(Action::Toggle),
            Some(Command::Choose)
        ));
        assert!(model.request(Action::Toggle).is_none());
        model.event(Event::Initialized);
        assert_eq!(model.state, State::Selecting);
        assert!(matches!(
            model.event(Event::Ready("Window".into())),
            Some(Command::Record)
        ));
        assert!(matches!(model.request(Action::Toggle), Some(Command::Stop)));
        model.event(Event::Recording {
            encoder: "H264".into(),
        });
        assert_eq!(model.state, State::Saving);
        assert!(model.request(Action::Toggle).is_none());
    }
    #[test]
    fn cancellation_or_quit_prevents_automatic_recording() {
        for action in [Action::Cancel, Action::Quit] {
            let mut model = Model::default();
            model.request(Action::Start);
            model.request(action);
            assert!(model.event(Event::Ready("Window".into())).is_none());
        }
    }
    #[test]
    fn window_selection_requires_record_and_preserves_last_video() {
        let mut model = Model {
            saved: Some("last.mkv".into()),
            ..Model::default()
        };
        model.request(Action::Choose);
        assert!(model.event(Event::Ready("Window".into())).is_none());
        assert_eq!(model.state, State::Ready);
        assert!(model.saved.is_some());
        assert!(matches!(
            model.request(Action::Record),
            Some(Command::Record)
        ));
    }
}
