use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{gio, glib, prelude::*};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    audio, audio_settings,
    model::{Action, Model, State, format_elapsed},
    status,
    worker::{self, Command, Event},
};

const APP_ID: &str = "io.github.mentioum.MentarchRecorder";

struct View {
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    detail: gtk::Label,
    timer: gtk::Label,
    status: gtk::Label,
    spinner: gtk::Spinner,
    primary: gtk::Button,
    secondary: gtk::Button,
    actions: gtk::Box,
    open: gtk::Button,
    folder: gtk::Button,
    settings: gtk::Button,
    microphone: gtk::CheckButton,
    desktop: gtk::CheckButton,
    audio_detail: gtk::Label,
}

struct Controller {
    view: View,
    model: RefCell<Model>,
    commands: UnboundedSender<Command>,
    app: gtk::Application,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    status_error_reported: Cell<bool>,
    notice: RefCell<Option<String>>,
    settings_window: glib::WeakRef<gtk::ApplicationWindow>,
    config_error: RefCell<Option<String>>,
}

pub fn run(initial: Action) {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(build);
    app.connect_activate(|app| app.activate_action("show", None));
    app.connect_command_line(|app, line| {
        let args = line.arguments();
        let action = args
            .get(1)
            .and_then(|a| Action::parse(&a.to_string_lossy()))
            .unwrap_or(Action::Show);
        app.activate_action(action.name(), None);
        glib::ExitCode::SUCCESS
    });
    app.run_with_args(&["mentarch-recorder", initial.name()]);
}

fn make_view(app: &gtk::Application) -> View {
    let css = gtk::CssProvider::new();
    css.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("A display is required"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Window Recorder")
        .default_width(400)
        .resizable(false)
        .icon_name(APP_ID)
        .build();
    window.add_css_class("recorder-window");
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some("Window Recorder"))));
    let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings.set_tooltip_text(Some("Audio settings and device meters"));
    header.pack_start(&settings);
    window.set_titlebar(Some(&header));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(24);
    content.set_margin_end(24);
    let icon = gtk::Image::from_icon_name("camera-video-symbolic");
    icon.set_pixel_size(32);
    icon.add_css_class("accent-icon");
    content.append(&icon);
    let title = gtk::Label::new(Some("One window. One recording."));
    title.add_css_class("title-2");
    content.append(&title);
    let detail = gtk::Label::new(None);
    detail.set_wrap(true);
    detail.set_max_width_chars(46);
    detail.set_justify(gtk::Justification::Center);
    detail.set_selectable(true);
    detail.add_css_class("description");
    content.append(&detail);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("recording-card");
    let timer = gtk::Label::new(Some("00:00"));
    timer.add_css_class("timer");
    card.append(&timer);
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_row.set_halign(gtk::Align::Center);
    let spinner = gtk::Spinner::new();
    status_row.append(&spinner);
    let status = gtk::Label::new(Some("Ready when you are"));
    status.add_css_class("dim-label");
    status_row.append(&status);
    card.append(&status_row);
    content.append(&card);

    let audio_row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    audio_row.set_halign(gtk::Align::Center);
    let microphone = gtk::CheckButton::with_label("Microphone");
    let desktop = gtk::CheckButton::with_label("Desktop audio");
    audio_row.append(&microphone);
    audio_row.append(&desktop);
    content.append(&audio_row);
    let audio_detail = gtk::Label::new(None);
    audio_detail.add_css_class("caption");
    audio_detail.add_css_class("dim-label");
    audio_detail.set_wrap(true);
    content.append(&audio_detail);

    let primary = gtk::Button::with_mnemonic("_Choose window");
    primary.add_css_class("suggested-action");
    primary.add_css_class("pill");
    primary.set_height_request(46);
    content.append(&primary);
    let secondary = gtk::Button::with_label("Cancel");
    secondary.add_css_class("flat");
    content.append(&secondary);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    actions.set_homogeneous(true);
    let open = gtk::Button::with_label("Open video");
    let folder = gtk::Button::with_label("Show folder");
    actions.append(&open);
    actions.append(&folder);
    content.append(&actions);
    let footer = gtk::Label::new(Some(
        "~/Videos/Recordings\nClose hides this window · Ctrl+Q quits",
    ));
    footer.add_css_class("dim-label");
    footer.add_css_class("caption");
    footer.set_justify(gtk::Justification::Center);
    content.append(&footer);
    window.set_child(Some(&content));
    // Keep the selectable description from receiving GTK's initial focus and
    // appearing fully highlighted when the controls first open.
    gtk::prelude::GtkWindowExt::set_focus(&window, Some(&primary));

    View {
        window,
        title,
        detail,
        timer,
        status,
        spinner,
        primary,
        secondary,
        actions,
        open,
        folder,
        settings,
        microphone,
        desktop,
        audio_detail,
    }
}

fn build(app: &gtk::Application) {
    let view = make_view(app);
    let (commands, events) = worker::spawn();
    let mut model = Model::new(status::latest_video());
    let initial_notice = match audio::Settings::load() {
        Ok(settings) => {
            model.audio = settings;
            None
        }
        Err(error) => Some(format!(
            "Audio settings could not be loaded; recording is video only. {error:#}"
        )),
    };
    let controller = Rc::new(Controller {
        view,
        model: RefCell::new(model),
        commands,
        app: app.clone(),
        hold: RefCell::new(Some(app.hold())),
        status_error_reported: Cell::new(false),
        notice: RefCell::new(initial_notice.clone()),
        config_error: RefCell::new(initial_notice),
        settings_window: glib::WeakRef::new(),
    });
    for action in [
        Action::Show,
        Action::Settings,
        Action::ToggleMic,
        Action::ToggleDesktop,
        Action::StartSilent,
        Action::StartMic,
        Action::StartDesktop,
        Action::StartBoth,
        Action::Background,
        Action::Toggle,
        Action::Start,
        Action::Choose,
        Action::Record,
        Action::Stop,
        Action::Cancel,
        Action::Open,
        Action::Folder,
        Action::Quit,
    ] {
        let command = gio::SimpleAction::new(action.name(), None);
        let weak = Rc::downgrade(&controller);
        command.connect_activate(move |_, _| {
            if let Some(controller) = weak.upgrade() {
                controller.dispatch(action);
            }
        });
        app.add_action(&command);
    }
    app.set_accels_for_action("app.quit", &["<Primary>q"]);
    {
        let weak = Rc::downgrade(&controller);
        controller.view.primary.connect_clicked(move |_| {
            if let Some(controller) = weak.upgrade() {
                let state = controller.model.borrow().state;
                controller.dispatch(
                    if matches!(state, State::Idle | State::Saved | State::Error) {
                        Action::Choose
                    } else {
                        Action::Toggle
                    },
                );
            }
        });
    }
    for (button, action) in [
        (&controller.view.secondary, Action::Cancel),
        (&controller.view.open, Action::Open),
        (&controller.view.folder, Action::Folder),
        (&controller.view.settings, Action::Settings),
    ] {
        let weak = Rc::downgrade(&controller);
        button.connect_clicked(move |_| {
            if let Some(controller) = weak.upgrade() {
                controller.dispatch(action);
            }
        });
    }
    for (button, action) in [
        (&controller.view.microphone, Action::ToggleMic),
        (&controller.view.desktop, Action::ToggleDesktop),
    ] {
        let weak = Rc::downgrade(&controller);
        button.connect_toggled(move |button| {
            if let Some(controller) = weak.upgrade() {
                let enabled = if action == Action::ToggleMic {
                    controller.model.borrow().audio.microphone
                } else {
                    controller.model.borrow().audio.desktop
                };
                if button.is_active() != enabled {
                    controller.dispatch(action);
                }
            }
        });
    }
    // The window is an optional control surface. Closing it never interrupts a
    // Waybar recording; Quit is an explicit action and always finalizes first.
    controller.view.window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
    controller.refresh();
    let mut published = Instant::now();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let mut changed = false;
        while let Ok(event) = events.try_recv() {
            let important = !matches!(event, Event::Tick { .. } | Event::Initialized);
            let finish_picker = matches!(event, Event::FinishPicker);
            let failed = matches!(event, Event::Error(_));
            let saved = matches!(event, Event::Saved { .. });
            let command = controller.model.borrow_mut().event(event);
            if important {
                controller.notice.borrow_mut().take();
            }
            if let Some(command) = command {
                controller.send(command);
            }
            if finish_picker || failed {
                controller.view.window.present();
            }
            if saved && !controller.view.window.is_visible() && !controller.model.borrow().closing {
                let notification = gio::Notification::new("Recording saved");
                notification.set_body(Some(&controller.model.borrow().detail));
                notification.set_default_action("app.open");
                controller
                    .app
                    .send_notification(Some("recording-saved"), &notification);
            }
            changed |= important;
        }
        {
            let model = controller.model.borrow();
            controller.render();
            if !model.worker_alive && model.closing {
                let _ = status::publish(&model, true);
                controller.hold.borrow_mut().take();
                controller.app.quit();
                return glib::ControlFlow::Break;
            }
        }
        if changed || published.elapsed() >= Duration::from_secs(1) {
            controller.publish();
            published = Instant::now();
        }
        glib::ControlFlow::Continue
    });
}

impl Controller {
    fn dispatch(self: &Rc<Self>, action: Action) {
        if matches!(
            action,
            Action::Toggle | Action::Start | Action::Choose | Action::Record
        ) && let Some(error) = self.config_error.borrow().as_ref()
        {
            self.show_notice(format!(
                "{error}\nOpen Audio settings and save your choices before recording."
            ));
            return;
        }
        match action {
            Action::Show => self.view.window.present(),
            Action::Settings => {
                if let Some(window) = self.settings_window.upgrade().filter(|w| w.is_visible()) {
                    window.present();
                } else {
                    let weak = Rc::downgrade(self);
                    let window = audio_settings::show(
                        &self.app,
                        self.model.borrow().audio.clone(),
                        move |settings| {
                            let controller = weak
                                .upgrade()
                                .ok_or_else(|| anyhow::anyhow!("Recorder has closed"))?;
                            controller.apply_audio(settings)
                        },
                    );
                    self.settings_window.set(Some(&window));
                }
            }
            Action::ToggleMic | Action::ToggleDesktop => {
                let mut settings = self.model.borrow().audio.clone();
                if action == Action::ToggleMic {
                    settings.microphone = !settings.microphone;
                } else {
                    settings.desktop = !settings.desktop;
                }
                if let Err(error) = self.apply_audio(settings) {
                    self.show_notice(format!("Could not update audio options: {error:#}"));
                }
            }
            Action::StartSilent | Action::StartMic | Action::StartDesktop | Action::StartBoth => {
                if matches!(
                    self.model.borrow().state,
                    State::Idle | State::Saved | State::Error | State::Ready
                ) {
                    let mut settings = self.model.borrow().audio.clone();
                    settings.microphone = matches!(action, Action::StartMic | Action::StartBoth);
                    settings.desktop = matches!(action, Action::StartDesktop | Action::StartBoth);
                    match self.apply_audio(settings) {
                        Ok(()) => self.dispatch(Action::Start),
                        Err(error) => {
                            self.show_notice(format!("Could not update audio options: {error:#}"))
                        }
                    }
                }
            }
            Action::Background => {}
            Action::Open | Action::Folder => {
                self.open(action == Action::Folder);
            }
            Action::Quit if !self.model.borrow().worker_alive => {
                let _ = status::publish(&self.model.borrow(), true);
                self.hold.borrow_mut().take();
                self.app.quit();
            }
            _ => {
                let command = self.model.borrow_mut().request(action);
                if let Some(command) = command {
                    self.notice.borrow_mut().take();
                    self.send(command);
                }
                self.refresh();
            }
        }
    }

    fn apply_audio(&self, settings: audio::Settings) -> anyhow::Result<()> {
        settings.save()?;
        self.config_error.borrow_mut().take();
        self.notice.borrow_mut().take();
        self.model.borrow_mut().audio = settings.clone();
        self.refresh();
        audio::write_menu(&settings, true)
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            let mut model = self.model.borrow_mut();
            model.event(Event::Error(
                "The recording worker stopped. Quit the recorder and open it again.".into(),
            ));
            model.worker_alive = false;
            self.view.window.present();
        }
    }

    fn refresh(&self) {
        self.render();
        self.publish();
    }

    fn render(&self) {
        self.view.render(&self.model.borrow());
        if let Some(notice) = self.notice.borrow().as_ref() {
            self.view.detail.set_text(notice);
        }
    }

    fn show_notice(&self, message: String) {
        *self.notice.borrow_mut() = Some(message);
        self.render();
        self.view.window.present();
    }

    fn publish(&self) {
        if let Err(error) = status::publish(&self.model.borrow(), false)
            && !self.status_error_reported.replace(true)
        {
            eprintln!("Cannot publish Waybar status: {error:#}");
            self.show_notice(format!(
                "Cannot update the Waybar recording indicator: {error:#}"
            ));
        }
    }

    fn open(self: &Rc<Self>, folder: bool) {
        let path = if folder {
            crate::output::recordings_dir().ok()
        } else {
            self.model
                .borrow()
                .saved
                .clone()
                .or_else(status::latest_video)
        };
        let Some(path) = path else {
            self.show_notice(
                "No saved recordings yet. Choose a window to make your first video.".into(),
            );
            return;
        };
        if folder {
            let _ = std::fs::create_dir_all(&path);
        }
        let file = gio::File::for_path(path);
        let weak = Rc::downgrade(self);
        gio::AppInfo::launch_default_for_uri_async(
            &file.uri(),
            None::<&gio::AppLaunchContext>,
            None::<&gio::Cancellable>,
            move |result| {
                if let Err(error) = result
                    && let Some(controller) = weak.upgrade()
                {
                    controller.show_notice(format!("Could not open the file: {error}"));
                }
            },
        );
    }
}

impl View {
    fn render(&self, model: &Model) {
        self.microphone.set_active(model.audio.microphone);
        self.desktop.set_active(model.audio.desktop);
        let active = matches!(
            model.state,
            State::Starting | State::Recording | State::Saving
        );
        self.audio_detail.set_text(&if active {
            format!(
                "Capturing: {} · Changes apply next time",
                model.active_audio.summary()
            )
        } else {
            format!("Next recording: {}", model.audio.summary())
        });
        let (title, detail, button, status) = match model.state {
            State::Idle => (
                "One window. One recording.",
                "Choose a window, then press Record. Your desktop stays out of the picture.".into(),
                "_Choose window",
                "Ready when you are",
            ),
            State::Selecting => (
                "Choose your window",
                if model.detail.is_empty() {
                    "Select one window, or Cancel, in the system sharing dialog.".into()
                } else {
                    model.detail.clone()
                },
                "Choosing window…",
                "Waiting for selection",
            ),
            State::Ready => (
                "Ready to record",
                model.target.clone(),
                "_Record",
                "Window selected",
            ),
            State::Starting => (
                "Starting your recording",
                model.target.clone(),
                "_Stop & save",
                "Waiting for the first frame",
            ),
            State::Recording => (
                "Recording your window",
                format!("{}\n{}", model.target, model.encoder),
                "_Stop & save",
                "● Recording",
            ),
            State::Saving => (
                "Saving your recording",
                "Finishing your video…".into(),
                "Saving…",
                "Finalizing video",
            ),
            State::Saved => (
                "Your video is ready",
                model.detail.clone(),
                "_Record another window",
                "Saved successfully",
            ),
            State::Error => (
                "Something needs attention",
                model.detail.clone(),
                "_Try again",
                "Recording stopped",
            ),
        };
        self.timer.set_text(&format_elapsed(model.elapsed));
        self.title.set_text(title);
        self.detail.set_text(&detail);
        self.primary.set_label(button);
        self.primary.set_use_underline(true);
        self.status
            .set_text(if model.state == State::Recording && model.stalled {
                "Waiting for window frames…"
            } else {
                status
            });
        self.primary.set_sensitive(
            !matches!(model.state, State::Selecting | State::Saving)
                && !model.closing
                && model.worker_alive,
        );
        self.secondary.set_visible(model.state == State::Ready);
        self.secondary.set_sensitive(!model.closing);
        self.actions.set_visible(model.state == State::Saved);
        self.spinner.set_spinning(matches!(
            model.state,
            State::Selecting | State::Starting | State::Saving
        ));
        self.spinner.set_visible(self.spinner.is_spinning());
        if matches!(model.state, State::Starting | State::Recording) {
            self.primary.remove_css_class("suggested-action");
            self.primary.add_css_class("destructive-action");
        } else {
            self.primary.remove_css_class("destructive-action");
            self.primary.add_css_class("suggested-action");
        }
    }
}
