use std::{
    os::fd::{AsRawFd, OwnedFd},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use gstreamer::{self as gst, prelude::*};

use crate::output::Output;

#[derive(Clone, Copy, Debug)]
pub struct Encoder {
    pub factory: &'static str,
    pub label: &'static str,
    pipeline: &'static str,
    format: &'static str,
}

const ENCODERS: &[Encoder] = &[
    Encoder {
        factory: "vah264enc",
        label: "H.264 · hardware",
        pipeline: "vah264enc bitrate=8000 ! h264parse",
        format: "NV12",
    },
    Encoder {
        factory: "nvh264enc",
        label: "H.264 · NVIDIA",
        pipeline: "nvh264enc bitrate=8000 ! h264parse",
        format: "NV12",
    },
    Encoder {
        factory: "x264enc",
        label: "H.264 · software",
        pipeline: "x264enc speed-preset=veryfast tune=zerolatency bitrate=8000 key-int-max=60 ! h264parse",
        format: "I420",
    },
    Encoder {
        factory: "openh264enc",
        label: "H.264 · software",
        pipeline: "openh264enc bitrate=8000000 ! h264parse",
        format: "I420",
    },
    Encoder {
        factory: "vp8enc",
        label: "VP8 · software",
        pipeline: "vp8enc deadline=1 cpu-used=4 target-bitrate=8000000 keyframe-max-dist=60",
        format: "I420",
    },
];

pub fn check_plugins() -> Result<()> {
    let required = [
        "pipewiresrc",
        "queue",
        "videoconvert",
        "videoscale",
        "videorate",
        "capsfilter",
        "matroskamux",
        "filesink",
        "videotestsrc",
        "fakesink",
    ];
    let missing: Vec<_> = required
        .into_iter()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect();
    if !missing.is_empty() {
        bail!(
            "Missing GStreamer plugins: {}. On Arch, install gst-plugin-pipewire, gst-plugins-base, gst-plugins-good, gst-plugins-bad and gst-plugins-ugly.",
            missing.join(", ")
        );
    }
    Ok(())
}

pub fn select_encoder() -> Result<Encoder> {
    select_encoder_from(ENCODERS)
}

fn select_encoder_from(candidates: &[Encoder]) -> Result<Encoder> {
    let mut failures = Vec::new();
    for encoder in candidates {
        if gst::ElementFactory::find(encoder.factory).is_none() {
            continue;
        }
        match probe_encoder(*encoder) {
            Ok(()) => return Ok(*encoder),
            Err(error) => failures.push(format!("{}: {error:#}", encoder.factory)),
        }
    }
    bail!(
        "No usable video encoder. Install gst-plugins-ugly (x264) or gst-plugins-good (VP8). {}",
        failures.join("; ")
    )
}

fn probe_encoder(encoder: Encoder) -> Result<()> {
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc num-buffers=3 ! video/x-raw,width=320,height=240,framerate=30/1 ! videoconvert ! video/x-raw,format={} ! {} ! fakesink", encoder.format, encoder.pipeline
    ))?.downcast::<gst::Pipeline>().map_err(|_| anyhow::anyhow!("Invalid encoder test pipeline"))?;
    let guard = PipelineGuard(pipeline);
    guard.0.set_state(gst::State::Playing)?;
    wait_for_eos(&guard.0, Duration::from_secs(4))
}

struct PipelineGuard(gst::Pipeline);
impl Drop for PipelineGuard {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

pub enum Update {
    Eos,
    Error(String),
}

pub struct Recording {
    pipeline: PipelineGuard,
    output: Option<Output>,
    // pipewiresrc borrows the descriptor. It must outlive the pipeline.
    _remote: Option<OwnedFd>,
    started: Instant,
    frames: Arc<AtomicU64>,
    last_frame_ms: Arc<AtomicU64>,
}

/// Keep a stable, even-sized encoding canvas when the selected window resizes.
pub fn canvas(size: Option<(i32, i32)>) -> (i32, i32) {
    let (width, height) = size
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((1920, 1080));
    let scale = (1920.0 / width as f64).min(1080.0 / height as f64).min(1.0);
    (
        ((width as f64 * scale) as i32 / 2 * 2).max(2),
        ((height as f64 * scale) as i32 / 2 * 2).max(2),
    )
}

impl Recording {
    pub fn start(
        remote: OwnedFd,
        node: u32,
        size: Option<(i32, i32)>,
        encoder: Encoder,
        directory: &Path,
    ) -> Result<Self> {
        let source = gst::ElementFactory::make("pipewiresrc")
            .name("capture")
            .property("fd", remote.as_raw_fd())
            .property("path", node.to_string())
            .property("do-timestamp", true)
            .build()
            .context("Could not create the PipeWire source")?;
        // Do not retain portal-owned PipeWire buffers in the encoding queue.
        if source.find_property("use-bufferpool").is_some() {
            source.set_property("use-bufferpool", false);
        } else if source.find_property("always-copy").is_some() {
            source.set_property("always-copy", true);
        }
        // Leave on-disconnect at its default. PipeWire removes buffers during
        // resize/format negotiation too; its "eos" mode flushes the entire
        // pipeline, which can truncate an already-written Matroska file.
        // Portal Closed and bus errors handle actual capture loss instead.
        Self::build(source, Some(remote), size, encoder, directory)
    }

    fn build(
        source: gst::Element,
        remote: Option<OwnedFd>,
        size: Option<(i32, i32)>,
        encoder: Encoder,
        directory: &Path,
    ) -> Result<Self> {
        let output = Output::reserve(directory)?;
        let (width, height) = canvas(size);
        let description = format!(
            "queue name=input max-size-buffers=8 max-size-bytes=0 max-size-time=0 ! videoconvert ! videoscale add-borders=true ! videorate ! video/x-raw,format={},width={width},height={height},framerate=30/1,pixel-aspect-ratio=1/1 ! {} ! identity name=encoded ! matroskamux ! filesink name=output sync=false",
            encoder.format, encoder.pipeline
        );
        let pipeline = gst::parse::launch(&description)?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow::anyhow!("Could not construct recording pipeline"))?;
        let guard = PipelineGuard(pipeline);
        guard
            .0
            .by_name("output")
            .context("Missing output element")?
            .set_property(
                "location",
                output
                    .partial
                    .to_str()
                    .context("Output path is not valid UTF-8")?,
            );
        guard.0.add(&source)?;
        source.link(&guard.0.by_name("input").context("Missing input queue")?)?;

        let started = Instant::now();
        let frames = Arc::new(AtomicU64::new(0));
        let last_frame_ms = Arc::new(AtomicU64::new(0));
        let counter = frames.clone();
        let last = last_frame_ms.clone();
        guard
            .0
            .by_name("encoded")
            .unwrap()
            .static_pad("src")
            .unwrap()
            .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                counter.fetch_add(1, Ordering::Relaxed);
                last.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                gst::PadProbeReturn::Ok
            });
        guard
            .0
            .set_state(gst::State::Playing)
            .context("Could not start recording")?;
        Ok(Self {
            pipeline: guard,
            output: Some(output),
            _remote: remote,
            started,
            frames,
            last_frame_ms,
        })
    }

    pub fn has_frames(&self) -> bool {
        self.frames.load(Ordering::Relaxed) > 0
    }
    pub fn stalled(&self) -> bool {
        self.started.elapsed().as_millis() as u64
            > self.last_frame_ms.load(Ordering::Relaxed) + 5000
    }
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn poll(&self) -> Option<Update> {
        let bus = self.pipeline.0.bus().unwrap();
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::Eos(_) => return Some(Update::Eos),
                gst::MessageView::Error(error) => {
                    return Some(Update::Error(format!(
                        "{}: {}",
                        error
                            .src()
                            .map(|s| s.name().to_string())
                            .unwrap_or_default(),
                        error.error()
                    )));
                }
                _ => {}
            }
        }
        None
    }

    pub fn finish(mut self, already_eos: bool) -> Result<PathBuf> {
        let partial = self.output.as_ref().unwrap().partial.clone();
        let result = (|| {
            if !already_eos {
                // Inject at the queue, so a closed/disconnected PipeWire source
                // cannot prevent the encoder and muxer from draining.
                let queue = self.pipeline.0.by_name("input").unwrap();
                if !queue.send_event(gst::event::Eos::new()) {
                    bail!("The encoder refused to stop");
                }
                wait_for_eos(&self.pipeline.0, Duration::from_secs(10))?;
            }
            self.pipeline.0.set_state(gst::State::Null)?;
            if !self.has_frames() {
                bail!("The selected window did not produce any video frames");
            }
            verify_matroska_header(&partial)?;
            self.output.take().unwrap().finish()
        })();
        result.with_context(|| {
            format!(
                "Could not finalize the video. Any recoverable data is at {}",
                partial.display()
            )
        })
    }
}

fn verify_matroska_header(path: &Path) -> Result<()> {
    use std::io::Read;
    let mut header = [0; 4];
    std::fs::File::open(path)?.read_exact(&mut header)?;
    if header != [0x1a, 0x45, 0xdf, 0xa3] {
        bail!("The encoder produced an invalid Matroska header");
    }
    Ok(())
}

fn wait_for_eos(pipeline: &gst::Pipeline, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let bus = pipeline.bus().context("Pipeline has no message bus")?;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Some(message) =
            bus.timed_pop(gst::ClockTime::from_nseconds(remaining.as_nanos() as u64))
        {
            match message.view() {
                gst::MessageView::Eos(_) => return Ok(()),
                gst::MessageView::Error(error) => bail!("{} ({:?})", error.error(), error.debug()),
                _ => {}
            }
        }
    }
    bail!("Timed out waiting for the encoder to finish")
}

pub fn self_test(directory: &Path) -> Result<PathBuf> {
    let encoder = select_encoder()?;
    println!("Testing {}", encoder.factory);
    let source = gst::ElementFactory::make("videotestsrc")
        .property("num-buffers", 90i32)
        .build()?;
    let recording = Recording::build(source, None, Some((640, 360)), encoder, directory)?;
    wait_for_eos(&recording.pipeline.0, Duration::from_secs(15))?;
    recording.finish(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canvas_handles_odd_sizes_resize_and_hidpi() {
        assert_eq!(canvas(Some((801, 603))), (800, 602));
        assert_eq!(canvas(Some((3840, 2160))), (1920, 1080));
        assert_eq!(canvas(Some((1, 1))), (2, 2));
        assert_eq!(canvas(Some((0, -1))), (1920, 1080));
    }

    #[test]
    fn truncated_or_flushed_containers_cannot_be_reported_as_saved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.mkv.part");
        std::fs::write(&file, [0; 64]).unwrap();
        assert!(verify_matroska_header(&file).is_err());
        std::fs::write(&file, []).unwrap();
        assert!(verify_matroska_header(&file).is_err());
    }

    #[test]
    #[ignore = "requires installed GStreamer recording plugins"]
    fn encodes_and_finalizes_a_playable_video() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = self_test(dir.path()).unwrap();
        let pipeline =
            gst::parse::launch("filesrc name=input ! matroskademux ! decodebin ! fakesink")
                .unwrap()
                .downcast::<gst::Pipeline>()
                .unwrap();
        let guard = PipelineGuard(pipeline);
        guard
            .0
            .by_name("input")
            .unwrap()
            .set_property("location", file.to_str().unwrap());
        guard.0.set_state(gst::State::Playing).unwrap();
        wait_for_eos(&guard.0, Duration::from_secs(10)).unwrap();
        assert!(!dir.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "part")
        }));
    }

    #[test]
    #[ignore = "requires installed GStreamer recording plugins"]
    fn live_resize_and_explicit_stop_finalize() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = gst::parse::bin_from_description(
            "videotestsrc is-live=true ! capsfilter name=resize caps=video/x-raw,width=641,height=361,framerate=30/1", true).unwrap();
        let resize = source.by_name("resize").unwrap();
        let recording = Recording::build(
            source.upcast(),
            None,
            Some((641, 361)),
            select_encoder().unwrap(),
            dir.path(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert!(recording.has_frames());
        resize.set_property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("width", 803i32)
                .field("height", 605i32)
                .field("framerate", gst::Fraction::new(30, 1))
                .build(),
        );
        std::thread::sleep(Duration::from_millis(400));
        assert!(recording.poll().is_none());
        let file = recording.finish(false).unwrap();
        assert!(file.metadata().unwrap().len() > 1000);
        let pipeline =
            gst::parse::launch("filesrc name=input ! matroskademux ! decodebin ! fakesink")
                .unwrap()
                .downcast::<gst::Pipeline>()
                .unwrap();
        let guard = PipelineGuard(pipeline);
        guard
            .0
            .by_name("input")
            .unwrap()
            .set_property("location", file.to_str().unwrap());
        guard.0.set_state(gst::State::Playing).unwrap();
        wait_for_eos(&guard.0, Duration::from_secs(10)).unwrap();
    }

    #[test]
    #[ignore = "requires installed GStreamer recording plugins"]
    fn encoder_and_output_errors_do_not_leave_empty_files() {
        gst::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = gst::ElementFactory::make("videotestsrc").build().unwrap();
        let broken = Encoder {
            factory: "missing-encoder",
            label: "Broken",
            pipeline: "mentarch-missing-encoder",
            format: "I420",
        };
        assert!(Recording::build(source, None, None, broken, dir.path()).is_err());
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
        let occupied = dir.path().join("occupied");
        std::fs::write(&occupied, b"file").unwrap();
        let source = gst::ElementFactory::make("videotestsrc").build().unwrap();
        assert!(
            Recording::build(source, None, None, select_encoder().unwrap(), &occupied).is_err()
        );
    }

    #[test]
    #[ignore = "requires installed GStreamer recording plugins"]
    fn a_present_but_incompatible_encoder_falls_back() {
        gst::init().unwrap();
        let working = select_encoder().unwrap();
        let incompatible = Encoder {
            label: "Incompatible",
            pipeline: "audioconvert ! audio/x-raw",
            ..working
        };
        assert!(probe_encoder(incompatible).is_err());
        let selected = select_encoder_from(&[incompatible, working]).unwrap();
        assert_eq!(selected.label, working.label);
    }
}
