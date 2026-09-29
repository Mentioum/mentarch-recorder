use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

use anyhow::Result;
use gtk::{glib, prelude::*};

use crate::audio::{Device, Inventory, Preview, PreviewEvent, Settings};

struct MeterRow {
    bar: gtk::ProgressBar,
    level: gtk::Label,
}

struct Editor {
    window: gtk::ApplicationWindow,
    microphone: gtk::CheckButton,
    desktop: gtk::CheckButton,
    microphone_list: gtk::Box,
    desktop_list: gtk::Box,
    message: gtk::Label,
    selection: RefCell<Settings>,
    rows: RefCell<HashMap<String, MeterRow>>,
    preview: RefCell<Option<Preview>>,
    events: RefCell<Option<mpsc::Receiver<PreviewEvent>>>,
    closed: Cell<bool>,
}

fn description(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.add_css_class("description");
    label
}

pub fn show(
    app: &gtk::Application,
    settings: Settings,
    save: impl Fn(Settings) -> Result<()> + 'static,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Audio settings")
        .default_width(570)
        .default_height(640)
        .build();
    window.add_css_class("recorder-window");
    window.add_css_class("audio-settings");
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some("Audio settings"))));
    window.set_titlebar(Some(&header));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    for setter in [
        gtk::Widget::set_margin_top,
        gtk::Widget::set_margin_bottom,
        gtk::Widget::set_margin_start,
        gtk::Widget::set_margin_end,
    ] {
        setter(content.upcast_ref(), 20);
    }
    let hint = description(
        "Choose audio for your next recording. Enable either source, both, or neither for a silent video.",
    );
    content.append(&hint);
    let toggles = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    let microphone = gtk::CheckButton::with_label("Record microphone");
    microphone.set_active(settings.microphone);
    let desktop = gtk::CheckButton::with_label("Record desktop audio");
    desktop.set_active(settings.desktop);
    toggles.append(&microphone);
    toggles.append(&desktop);
    content.append(&toggles);

    let lists = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let microphone_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let desktop_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for (title, list) in [
        ("Microphone", &microphone_list),
        ("Desktop output", &desktop_list),
    ] {
        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("section-title");
        heading.set_xalign(0.0);
        lists.append(&heading);
        lists.append(list);
        list.append(&description("Finding devices…"));
    }
    lists.append(&description(
        "Desktop audio includes everything playing through the selected output.",
    ));
    let scroll = gtk::ScrolledWindow::builder()
        .child(&lists)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(260)
        .build();
    content.append(&scroll);
    content.append(&description("Speak or play sound to identify devices. These meters listen only while settings is open; preview audio is never saved or played back."));
    let message = description("");
    message.set_visible(false);
    content.append(&message);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let refresh = gtk::Button::with_label("Refresh devices");
    let apply = gtk::Button::with_label("Save settings");
    apply.add_css_class("suggested-action");
    apply.set_hexpand(true);
    actions.append(&refresh);
    actions.append(&apply);
    content.append(&actions);
    window.set_child(Some(&content));

    let editor = Rc::new(Editor {
        window: window.clone(),
        microphone,
        desktop,
        microphone_list,
        desktop_list,
        message,
        selection: RefCell::new(settings),
        rows: RefCell::new(HashMap::new()),
        preview: RefCell::new(None),
        events: RefCell::new(None),
        closed: Cell::new(false),
    });
    let weak = Rc::downgrade(&editor);
    refresh.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.start_preview();
        }
    });
    let weak = Rc::downgrade(&editor);
    apply.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            let mut settings = editor.selection.borrow().clone();
            settings.microphone = editor.microphone.is_active();
            settings.desktop = editor.desktop.is_active();
            match save(settings) {
                Ok(()) => editor.window.close(),
                Err(error) => {
                    editor
                        .message
                        .set_text(&format!("Could not save settings: {error:#}"));
                    editor.message.set_visible(true);
                }
            }
        }
    });
    let weak = Rc::downgrade(&editor);
    window.connect_close_request(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.closed.set(true);
            editor.preview.borrow_mut().take();
            editor.events.borrow_mut().take();
        }
        glib::Propagation::Proceed
    });
    editor.start_preview();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if editor.closed.get() {
            return glib::ControlFlow::Break;
        }
        let events: Vec<_> = editor
            .events
            .borrow()
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                PreviewEvent::Devices(inventory) => editor.devices(inventory),
                PreviewEvent::Levels(levels) => {
                    let rows = editor.rows.borrow();
                    for (id, peak, error) in levels {
                        if let Some(row) = rows.get(&id) {
                            row.bar.set_fraction(if error.is_some() {
                                0.0
                            } else {
                                (peak + 60.0) / 60.0
                            });
                            row.level.set_text(if error.is_some() {
                                "Unavailable"
                            } else if peak <= -60.0 {
                                "Quiet"
                            } else {
                                ""
                            });
                            if error.is_none() && peak > -60.0 {
                                row.level.set_text(&format!("{peak:.0} dB"));
                            }
                            row.bar.set_tooltip_text(error.as_deref());
                            if peak > -3.0 {
                                row.bar.add_css_class("loud");
                            } else {
                                row.bar.remove_css_class("loud");
                            }
                        }
                    }
                }
                PreviewEvent::Error(error) => {
                    editor.message.set_text(&error);
                    editor.message.set_visible(true);
                }
            }
        }
        glib::ControlFlow::Continue
    });
    window.present();
    window
}

impl Editor {
    fn start_preview(&self) {
        self.preview.borrow_mut().take();
        self.rows.borrow_mut().clear();
        let (preview, events) = Preview::start();
        *self.preview.borrow_mut() = Some(preview);
        *self.events.borrow_mut() = Some(events);
        self.message
            .set_text("Finding devices and starting level meters…");
        self.message.set_visible(true);
    }

    fn devices(self: &Rc<Self>, inventory: Inventory) {
        self.message.set_visible(false);
        self.section(
            &self.microphone_list,
            &inventory.microphones,
            &inventory.default_microphone,
            true,
        );
        self.section(
            &self.desktop_list,
            &inventory.desktops,
            &inventory.default_desktop,
            false,
        );
    }

    fn section(
        self: &Rc<Self>,
        list: &gtk::Box,
        devices: &[Device],
        default: &str,
        microphone: bool,
    ) {
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let selected = if microphone {
            self.selection.borrow().microphone_device.clone()
        } else {
            self.selection.borrow().desktop_device.clone()
        };
        let label = devices
            .iter()
            .find(|d| d.id == default)
            .map(|d| d.label.as_str())
            .unwrap_or("unavailable");
        let system = gtk::CheckButton::with_label(&format!("Follow system default · {label}"));
        system.set_active(selected.is_none());
        system.set_tooltip_text(Some(
            "Resolve the default device when each recording starts.",
        ));
        self.connect_selection(&system, None, microphone);
        list.append(&system);
        for device in devices {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 7);
            row.add_css_class("device-row");
            let choice = gtk::CheckButton::with_label(&device.label);
            choice.set_group(Some(&system));
            choice.set_active(selected.as_deref() == Some(device.id.as_str()));
            choice.set_tooltip_text(Some(&device.id));
            if let Some(label) = choice.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
                label.set_wrap(true);
            }
            self.connect_selection(&choice, Some(device.id.clone()), microphone);
            row.append(&choice);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let bar = gtk::ProgressBar::new();
            bar.set_hexpand(true);
            bar.set_valign(gtk::Align::Center);
            let level = gtk::Label::new(Some("Listening…"));
            level.set_width_chars(11);
            level.set_xalign(1.0);
            level.add_css_class("caption");
            line.append(&bar);
            line.append(&level);
            row.append(&line);
            if device.muted {
                row.append(&description("Muted in system audio settings"));
            }
            list.append(&row);
            self.rows
                .borrow_mut()
                .insert(device.source.clone(), MeterRow { bar, level });
        }
        if let Some(id) = selected.filter(|id| !devices.iter().any(|d| d.id == *id)) {
            let missing = gtk::CheckButton::with_label(
                "Saved device is disconnected · choose another device",
            );
            missing.set_group(Some(&system));
            missing.set_active(true);
            missing.set_tooltip_text(Some(&id));
            list.append(&missing);
        }
        if devices.is_empty() {
            list.append(&description(
                "No devices found. Connect a device, then refresh.",
            ));
        }
    }

    fn connect_selection(
        self: &Rc<Self>,
        button: &gtk::CheckButton,
        id: Option<String>,
        microphone: bool,
    ) {
        let weak = Rc::downgrade(self);
        button.connect_toggled(move |button| {
            if button.is_active()
                && let Some(editor) = weak.upgrade()
            {
                if microphone {
                    editor.selection.borrow_mut().microphone_device = id.clone();
                } else {
                    editor.selection.borrow_mut().desktop_device = id.clone();
                }
            }
        });
    }
}
