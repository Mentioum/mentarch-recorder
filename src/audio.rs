use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, path::PathBuf, process::Command, sync::mpsc, time::Duration};

use anyhow::{Context, Result, bail};
use gstreamer::{self as gst, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub microphone: bool,
    pub desktop: bool,
    pub microphone_device: Option<String>,
    pub desktop_device: Option<String>,
}

impl Settings {
    pub fn summary(&self) -> &'static str {
        match (self.microphone, self.desktop) {
            (false, false) => "Video only",
            (true, false) => "Microphone",
            (false, true) => "Desktop audio",
            (true, true) => "Microphone + desktop audio",
        }
    }

    pub fn load() -> Result<Self> {
        match fs::read(config_dir()?.join("audio.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Cannot read audio settings"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("Cannot read audio settings"),
        }
    }

    pub fn save(&self) -> Result<()> {
        atomic_write(
            &config_dir()?.join("audio.json"),
            &serde_json::to_vec_pretty(self)?,
        )?;
        Ok(())
    }

    pub fn resolve(&self) -> Result<Vec<Input>> {
        if !self.microphone && !self.desktop {
            return Ok(Vec::new());
        }
        check_plugins()?;
        self.resolve_with(&Inventory::discover()?)
    }

    fn resolve_with(&self, inventory: &Inventory) -> Result<Vec<Input>> {
        let mut inputs = Vec::new();
        for (enabled, selected, default, devices, kind) in [
            (
                self.microphone,
                &self.microphone_device,
                &inventory.default_microphone,
                &inventory.microphones,
                "Microphone",
            ),
            (
                self.desktop,
                &self.desktop_device,
                &inventory.default_desktop,
                &inventory.desktops,
                "Desktop audio",
            ),
        ] {
            if !enabled {
                continue;
            }
            let name = selected.as_deref().unwrap_or(default);
            let device = devices.iter().find(|d| d.id == name)
                .with_context(|| format!("{kind} device is unavailable. Open Audio settings and choose a connected device."))?;
            inputs.push(Input {
                source: device.source.clone(),
                label: format!("{kind}: {}", device.label),
            });
        }
        Ok(inputs)
    }
}

fn config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Configuration directory is unavailable")?
        .join("mentarch-recorder"))
}

pub fn menu_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("waybar-menu.ui"))
}

fn atomic_write(path: &std::path::Path, contents: &[u8]) -> Result<bool> {
    if fs::read(path).ok().as_deref() == Some(contents) {
        return Ok(false);
    }
    fs::create_dir_all(path.parent().context("Missing configuration directory")?)?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)?;
    Ok(true)
}

pub fn write_menu(settings: &Settings, reload: bool) -> Result<()> {
    let contents = include_str!("../data/waybar-menu.ui")
        .replace(
            "False</property><!--MIC-->",
            &format!("{}</property><!--MIC-->", settings.microphone),
        )
        .replace(
            "False</property><!--DESKTOP-->",
            &format!("{}</property><!--DESKTOP-->", settings.desktop),
        );
    let path = menu_path()?;
    let changed = atomic_write(&path, contents.as_bytes())?;
    // Waybar builds custom menus once. Refresh only when checkbox values change,
    // and only if its installed recorder module points at our generated menu.
    let integrated = dirs::config_dir()
        .and_then(|d| fs::read(d.join("waybar/mentarch-recorder.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|v| v["custom/recorder"]["menu-file"].as_str() == path.to_str());
    if changed && reload && integrated {
        Command::new("pkill")
            .args(["-USR2", "-x", "waybar"])
            .status()
            .context("Could not refresh Waybar's audio checkboxes")?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub source: String,
    pub label: String,
    pub muted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub microphones: Vec<Device>,
    pub desktops: Vec<Device>,
    pub default_microphone: String,
    pub default_desktop: String,
}

fn pactl(args: &[&str]) -> Result<Value> {
    let output = Command::new("timeout")
        .args(["3s", "pactl", "--format=json"])
        .args(args)
        .output()
        .context("Cannot list audio devices. Install libpulse (pactl) and run pipewire-pulse.")?;
    if !output.status.success() {
        bail!(
            "Audio service did not respond: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("Invalid audio device list")
}

impl Inventory {
    pub fn discover() -> Result<Self> {
        Self::from_json(
            &pactl(&["list", "sources"])?,
            &pactl(&["list", "sinks"])?,
            &pactl(&["info"])?,
        )
    }

    fn from_json(sources: &Value, sinks: &Value, info: &Value) -> Result<Self> {
        let sources = sources.as_array().context("Invalid microphone list")?;
        let sinks = sinks.as_array().context("Invalid output list")?;
        let monitors: Vec<&str> = sinks
            .iter()
            .filter_map(|s| s["monitor_source"].as_str())
            .collect();
        let microphones = sources
            .iter()
            .filter(|s| {
                let name = s["name"].as_str().unwrap_or_default();
                !monitors.contains(&name)
                    && s["properties"]["device.class"].as_str() != Some("monitor")
            })
            .filter_map(|s| {
                let id = s["name"].as_str()?.to_owned();
                Some(Device {
                    label: s["description"].as_str().unwrap_or(&id).into(),
                    source: id.clone(),
                    id,
                    muted: s["mute"].as_bool().unwrap_or(false),
                })
            })
            .collect();
        let desktops = sinks
            .iter()
            .filter_map(|s| {
                let id = s["name"].as_str()?.to_owned();
                let source = s["monitor_source"].as_str()?.to_owned();
                Some(Device {
                    label: s["description"].as_str().unwrap_or(&id).into(),
                    id,
                    source,
                    muted: s["mute"].as_bool().unwrap_or(false),
                })
            })
            .collect();
        Ok(Self {
            microphones,
            desktops,
            default_microphone: info["default_source_name"]
                .as_str()
                .unwrap_or_default()
                .into(),
            default_desktop: info["default_sink_name"]
                .as_str()
                .unwrap_or_default()
                .into(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Input {
    pub source: String,
    pub label: String,
}

pub fn check_plugins() -> Result<()> {
    for name in [
        "pulsesrc",
        "audioconvert",
        "audioresample",
        "audiomixer",
        "opusenc",
        "level",
    ] {
        if gst::ElementFactory::find(name).is_none() {
            bail!("Missing audio plugin {name}. Install gst-plugins-base and gst-plugins-good.");
        }
    }
    Ok(())
}

pub fn source(input: &Input) -> Result<gst::Element> {
    let source = gst::ElementFactory::make("pulsesrc")
        .property("device", &input.source)
        .property("client-name", "Window Recorder")
        .property("provide-clock", false)
        .property("do-timestamp", true)
        .property("volume", 1.0f64)
        .property_from_str("slave-method", "resample")
        .property(
            "stream-properties",
            gst::Structure::builder("props")
                .field("application.name", "Window Recorder")
                .field("media.name", &input.label)
                .field("node.dont-reconnect", "true")
                .field(
                    "module-stream-restore.id",
                    format!("mentarch-recorder-{}", input.source),
                )
                .build(),
        )
        .build()
        .with_context(|| format!("Cannot open {}", input.label))?;
    // Some Pulse servers move capture streams when a device disappears. Check
    // the route before forwarding buffers, including for live meter previews.
    let weak = source.downgrade();
    let expected = input.source.clone();
    let label = input.label.clone();
    let failed = AtomicBool::new(false);
    source.static_pad("src").context("Audio source has no output pad")?.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
        let Some(source) = weak.upgrade() else { return gst::PadProbeReturn::Drop; };
        if failed.load(Ordering::Relaxed) { return gst::PadProbeReturn::Drop; }
        if source.property::<Option<String>>("current-device").as_deref() != Some(expected.as_str()) {
            if !failed.swap(true, Ordering::Relaxed) {
                let _ = source.post_message(gst::message::Error::builder(gst::ResourceError::Read,
                    &format!("{label} disconnected or changed. Choose a connected device in Audio settings.")).src(&source).build());
            }
            return gst::PadProbeReturn::Drop;
        }
        gst::PadProbeReturn::Ok
    });
    Ok(source)
}

pub struct Meter {
    pipeline: gst::Pipeline,
    pub peak: f64,
    pub error: Option<String>,
}

impl Meter {
    pub fn new(input: &Input) -> Result<Self> {
        Self::from_source(source(input)?)
    }

    fn from_source(source: gst::Element) -> Result<Self> {
        let pipeline = gst::parse::launch(
            "audioconvert name=input ! level interval=100000000 ! fakesink sync=false",
        )?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("Invalid level meter pipeline"))?;
        let meter = Self {
            pipeline,
            peak: -60.0,
            error: None,
        };
        meter.pipeline.add(&source)?;
        source.link(&meter.pipeline.by_name("input").unwrap())?;
        meter.pipeline.set_state(gst::State::Playing)?;
        Ok(meter)
    }

    pub fn poll(&mut self) {
        while let Some(message) = self.pipeline.bus().unwrap().pop() {
            match message.view() {
                gst::MessageView::Element(e) => {
                    if let Some(s) = e.structure()
                        && s.name() == "level"
                        && let Ok(values) = s.get::<gst::glib::ValueArray>("peak")
                    {
                        self.peak = values
                            .iter()
                            .filter_map(|v| v.get::<f64>().ok())
                            .fold(-60.0, f64::max)
                            .clamp(-60.0, 0.0);
                    }
                }
                gst::MessageView::Error(e) => {
                    self.peak = -60.0;
                    self.error = Some(e.error().to_string());
                    let _ = self.pipeline.set_state(gst::State::Null);
                }
                _ => {}
            }
        }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub enum PreviewEvent {
    Devices(Inventory),
    Levels(Vec<(String, f64, Option<String>)>),
    Error(String),
}

pub struct Preview(mpsc::Sender<()>);

impl Preview {
    pub fn start() -> (Self, mpsc::Receiver<PreviewEvent>) {
        let (stop, commands) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                gst::init()?;
                check_plugins()?;
                let inventory = Inventory::discover()?;
                if !matches!(commands.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                    return Ok(());
                }
                let mut meters: Vec<_> = inventory
                    .microphones
                    .iter()
                    .chain(&inventory.desktops)
                    .map(|d| {
                        (
                            d.source.clone(),
                            Meter::new(&Input {
                                source: d.source.clone(),
                                label: format!("Level preview: {}", d.label),
                            }),
                        )
                    })
                    .collect();
                if events.send(PreviewEvent::Devices(inventory)).is_err() {
                    return Ok(());
                }
                while let Err(mpsc::RecvTimeoutError::Timeout) =
                    commands.recv_timeout(Duration::from_millis(100))
                {
                    let levels = meters
                        .iter_mut()
                        .map(|(name, meter)| match meter {
                            Ok(m) => {
                                m.poll();
                                (name.clone(), m.peak, m.error.clone())
                            }
                            Err(e) => (name.clone(), -60.0, Some(format!("{e:#}"))),
                        })
                        .collect();
                    if events.send(PreviewEvent::Levels(levels)).is_err() {
                        break;
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                let _ = events.send(PreviewEvent::Error(format!("{e:#}")));
            }
        });
        (Self(stop), receiver)
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_uses_output_monitor_and_never_substitutes_missing_devices() {
        let devices = Inventory::from_json(
            &serde_json::json!([{"name":"mic","description":"USB Mic"},{"name":"output.monitor","properties":{"device.class":"monitor"}}]),
            &serde_json::json!([{"name":"output","description":"Speakers","monitor_source":"output.monitor"}]),
            &serde_json::json!({"default_source_name":"mic","default_sink_name":"output"}),
        ).unwrap();
        assert_eq!(devices.microphones.len(), 1);
        let mut settings = Settings {
            microphone: true,
            desktop: true,
            ..Settings::default()
        };
        let inputs = settings.resolve_with(&devices).unwrap();
        assert_eq!(inputs[0].source, "mic");
        assert_eq!(inputs[1].source, "output.monitor");
        settings.microphone_device = Some("unplugged".into());
        assert!(settings.resolve_with(&devices).is_err());
        settings.microphone = false;
        assert_eq!(settings.resolve_with(&devices).unwrap().len(), 1);
    }

    #[test]
    #[ignore = "requires GStreamer audio plugins"]
    fn level_meter_reports_tone_and_stops_on_drop() {
        gst::init().unwrap();
        let source = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .property("volume", 0.5f64)
            .build()
            .unwrap();
        let mut meter = Meter::from_source(source).unwrap();
        std::thread::sleep(Duration::from_millis(350));
        meter.poll();
        assert!(meter.error.is_none());
        assert!((-8.0..-4.0).contains(&meter.peak), "{}", meter.peak);
        let pipeline = meter.pipeline.clone();
        drop(meter);
        assert_eq!(pipeline.current_state(), gst::State::Null);
    }
}
