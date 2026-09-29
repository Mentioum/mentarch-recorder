use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{
    audio,
    engine::{self, Encoder, Recording, Update},
    output,
    portal::{self, Portal, Target},
};

#[derive(Debug)]
pub enum Command {
    Choose,
    Record(audio::Settings),
    Stop,
    Cancel,
    Shutdown,
}

#[derive(Debug)]
pub enum Event {
    Initialized,
    Idle,
    Selecting,
    FinishPicker,
    Ready(String),
    Starting,
    Recording { encoder: String },
    Tick { elapsed: Duration, stalled: bool },
    Saving,
    Saved { path: PathBuf, note: Option<String> },
    Error(String),
    Shutdown,
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

pub fn spawn() -> (UnboundedSender<Command>, mpsc::Receiver<Event>) {
    let (commands, receiver) = unbounded_channel();
    let (events, output) = mpsc::channel();
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || {
            let result = (|| {
                gstreamer::init()?;
                runtime()?.block_on(run(receiver, &events))
            })();
            if let Err(error) = result {
                let _ = events.send(Event::Error(format!("{error:#}")));
            }
            let _ = events.send(Event::Shutdown);
        })
        .expect("Could not start the recording worker");
    (commands, output)
}

async fn run(mut commands: UnboundedReceiver<Command>, events: &mpsc::Sender<Event>) -> Result<()> {
    let emit = |event| {
        let _ = events.send(event);
    };
    let mut portal: Option<Portal> = None;
    let mut target: Option<Target> = None;
    let mut recording: Option<Recording> = None;
    let mut encoder: Option<Encoder> = None;
    let mut announced = false;
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    emit(Event::Initialized);

    loop {
        tokio::select! {
            command = commands.recv() => match command.unwrap_or(Command::Shutdown) {
                Command::Choose if recording.is_none() => {
                    target.take();
                    if let Some(old) = portal.take() { old.close().await; }
                    emit(Event::Selecting);
                    let setup = (|| { engine::check_plugins()?; engine::select_encoder() })();
                    let selected_encoder = match setup {
                        Ok(value) => value,
                        Err(error) => { emit(Event::Error(format!("{error:#}"))); continue; }
                    };
                    let created = tokio::time::timeout(Duration::from_secs(10), Portal::create()).await;
                    let session = match created {
                        Ok(Ok(session)) => session,
                        Ok(Err(error)) => { emit(Event::Error(format!("Cannot open window picker: {error:#}"))); continue; }
                        Err(_) => { emit(Event::Error("The ScreenCast portal did not respond. Check the portal service.".into())); continue; }
                    };
                    // XDPH 1.4 runs its picker synchronously in SelectSources
                    // and cannot process Session.Close until it returns. Keep
                    // awaiting the native Cancel/selection instead of dropping
                    // the request and orphaning a picker that blocks all apps.
                    let mut shutdown = false;
                    let mut discard = false;
                    let mut picking = Box::pin(session.pick());
                    let picked = loop {
                        tokio::select! {
                            result = &mut picking => break result,
                            command = commands.recv(), if !shutdown => {
                                match command {
                                    Some(Command::Shutdown) | None => shutdown = true,
                                    Some(Command::Cancel) => discard = true,
                                    _ => continue,
                                }
                                emit(Event::FinishPicker);
                            }
                        }
                    };
                    // End the borrow before moving the session into Ready.
                    drop(picking);
                    if shutdown || discard {
                        session.close().await;
                        if shutdown { break; }
                        emit(Event::Idle);
                        continue;
                    }
                    match picked {
                        Ok(chosen) => {
                            emit(Event::Ready(chosen.description.clone()));
                            target = Some(chosen);
                            portal = Some(session);
                            encoder = Some(selected_encoder);
                        }
                        Err(error) => {
                            session.close().await;
                            if portal::is_cancelled(&error) { emit(Event::Idle); }
                            else { emit(Event::Error(format!("Could not select a window: {error:#}"))); }
                        }
                    }
                }
                Command::Record(audio) if recording.is_none() && target.is_some() => {
                    emit(Event::Starting);
                    let chosen = target.take().unwrap();
                    let result = output::recordings_dir().and_then(|directory|
                        Recording::start(chosen.remote, chosen.node, chosen.size, encoder.unwrap(), &directory, &audio));
                    match result {
                        Ok(active) => { recording = Some(active); announced = false; }
                        Err(error) => {
                            if let Some(session) = portal.take() { session.close().await; }
                            emit(Event::Error(format!("Could not start recording: {error:#}")));
                        }
                    }
                }
                Command::Stop => {
                    if let Some(active) = recording.take() {
                        save(active, false, None, events);
                        if let Some(session) = portal.take() { session.close().await; }
                    }
                }
                Command::Cancel if recording.is_none() => {
                    target.take();
                    if let Some(session) = portal.take() { session.close().await; }
                    emit(Event::Idle);
                }
                Command::Shutdown => {
                    if let Some(active) = recording.take() { save(active, false, None, events); }
                    target.take();
                    if let Some(session) = portal.take() { session.close().await; }
                    break;
                }
                _ => {},
            },
            _ = interval.tick() => {
                let closed = portal.as_ref().is_some_and(Portal::is_closed);
                if let Some(active) = recording.as_ref() {
                    let update = active.poll();
                    if closed || update.is_some() {
                        let already_eos = matches!(update, Some(Update::Eos));
                        let note = match update {
                            Some(Update::Error(message)) => Some(format!("Capture ended: {message}")),
                            _ => Some("The selected window or capture session closed.".into()),
                        };
                        save(recording.take().unwrap(), already_eos, note, events);
                        if let Some(session) = portal.take() { session.close().await; }
                        continue;
                    }
                    if !announced && active.has_frames() {
                        announced = true;
                        emit(Event::Recording { encoder: encoder.unwrap().label.into() });
                    }
                    if !announced && active.elapsed() > Duration::from_secs(12) {
                        // No video was encoded; do not leave a source running.
                        drop(recording.take());
                        if let Some(session) = portal.take() { session.close().await; }
                        emit(Event::Error("No frames arrived from the selected window. Keep it visible and try again.".into()));
                        continue;
                    }
                    emit(Event::Tick { elapsed: active.elapsed(), stalled: active.stalled() });
                } else if closed {
                    target.take();
                    if let Some(session) = portal.take() { session.close().await; }
                    emit(Event::Error("The selected window or capture session closed. Choose a window again.".into()));
                }
            }
        }
    }
    Ok(())
}

fn save(
    recording: Recording,
    already_eos: bool,
    note: Option<String>,
    events: &mpsc::Sender<Event>,
) {
    let _ = events.send(Event::Saving);
    let event = match recording.finish(already_eos) {
        Ok(path) => Event::Saved { path, note },
        Err(error) => Event::Error(format!("{error:#}")),
    };
    let _ = events.send(event);
}

pub async fn capture_test() -> Result<()> {
    engine::check_plugins()?;
    let encoder = engine::select_encoder()?;
    let session = Portal::create().await?;
    let result = async {
        println!("Choose an individual window in the portal picker.");
        let target = session.pick().await?;
        println!(
            "Verified window source: node {}, {:?}",
            target.node, target.size
        );
        let active = Recording::start(
            target.remote,
            target.node,
            target.size,
            encoder,
            &output::recordings_dir()?,
            &audio::Settings::default(),
        )?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(update) = active.poll() {
                match update {
                    Update::Eos | Update::SourceEnded => {
                        println!(
                            "Saved {}",
                            active.finish(matches!(update, Update::Eos))?.display()
                        );
                        return Ok(());
                    }
                    Update::Error(error) => bail!("Capture failed: {error}"),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        println!(
            "Saved {}",
            active
                .finish(false)
                .context("Capture test could not save")?
                .display()
        );
        Ok(())
    }
    .await;
    session.close().await;
    result
}

#[cfg(all(test, feature = "live-portal-tests"))]
mod tests {
    use super::*;

    fn until(events: &mpsc::Receiver<Event>, matches: impl Fn(&Event) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let event = events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            assert!(!matches!(event, Event::Error(_)), "{event:?}");
            if matches(&event) {
                return;
            }
        }
    }

    #[test]
    #[ignore = "opens real portal pickers and cancels them; run explicitly in Hyprland"]
    fn live_portal_cancellation_repeat_and_shutdown() {
        let (commands, events) = spawn();
        until(&events, |event| matches!(event, Event::Initialized));
        for _ in 0..2 {
            commands.send(Command::Choose).unwrap();
            until(&events, |event| matches!(event, Event::Selecting));
            std::thread::sleep(Duration::from_secs(1));
            // Dismiss the native picker (manually or from the test harness).
            until(&events, |event| matches!(event, Event::Idle));
        }
        commands.send(Command::Choose).unwrap();
        until(&events, |event| matches!(event, Event::Selecting));
        std::thread::sleep(Duration::from_secs(1));
        commands.send(Command::Shutdown).unwrap();
        until(&events, |event| matches!(event, Event::FinishPicker));
        until(&events, |event| matches!(event, Event::Shutdown));
    }
}
