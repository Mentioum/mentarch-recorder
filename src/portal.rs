use std::{
    os::fd::OwnedFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use ashpd::desktop::{
    PersistMode, Session,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
};
use futures_util::StreamExt;

pub async fn check() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let portal = Screencast::new().await.context("Cannot connect to the ScreenCast portal")?;
        if !portal.available_source_types().await?.contains(SourceType::Window) {
            bail!("Your ScreenCast portal does not support individual windows. Start xdg-desktop-portal-hyprland in a Hyprland session.");
        }
        println!("ScreenCast portal supports individual windows.");
        Ok(())
    }).await.context("The ScreenCast portal did not respond")?
}

pub struct Portal {
    proxy: Screencast,
    session: Arc<Session<Screencast>>,
    closed: Arc<AtomicBool>,
    _monitor: Monitor,
}

struct Monitor(tokio::task::JoinHandle<()>);
impl Drop for Monitor {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct Target {
    pub remote: OwnedFd,
    pub node: u32,
    pub size: Option<(i32, i32)>,
    pub description: String,
}

impl Portal {
    pub async fn create() -> Result<Self> {
        // A separate connection gives each capture a bounded lifetime even if
        // the portal fails to answer Close during shutdown.
        let connection = ashpd::zbus::Connection::session().await?;
        let proxy = Screencast::with_connection(connection).await?;
        if !proxy
            .available_source_types()
            .await?
            .contains(SourceType::Window)
        {
            bail!("Window capture is unavailable. Check xdg-desktop-portal-hyprland.");
        }
        let session = Arc::new(proxy.create_session(Default::default()).await?);
        let closed = Arc::new(AtomicBool::new(false));
        let watched_session = session.clone();
        let flag = closed.clone();
        let (ready, subscribed) = tokio::sync::oneshot::channel();
        let monitor = Monitor(tokio::spawn(async move {
            match watched_session.receive_closed().await {
                Ok(mut signals) => {
                    let _ = ready.send(Ok(()));
                    signals.next().await;
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            }
            flag.store(true, Ordering::Relaxed);
        }));
        if let Err(error) = subscribed.await? {
            let _ = session.close().await;
            return Err(error.into());
        }
        Ok(Self {
            proxy,
            session,
            closed,
            _monitor: monitor,
        })
    }

    pub async fn pick(&self) -> Result<Target> {
        let modes = self.proxy.available_cursor_modes().await?;
        let cursor = if modes.contains(CursorMode::Embedded) {
            CursorMode::Embedded
        } else {
            CursorMode::Hidden
        };
        self.proxy
            .select_sources(
                &self.session,
                SelectSourcesOptions::default()
                    .set_sources(Some(SourceType::Window.into()))
                    .set_multiple(false)
                    .set_cursor_mode(cursor)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await?
            .response()?;
        let response = self
            .proxy
            .start(&self.session, None, Default::default())
            .await?
            .response()?;
        if response.streams().len() != 1 {
            bail!("The portal must return exactly one window");
        }
        let stream = &response.streams()[0];
        // Never silently record a monitor or a rectangle when a backend ignores
        // the requested source type.
        if stream.source_type() != Some(SourceType::Window) {
            bail!(
                "The portal did not return a verified window source. Choose an individual window."
            );
        }
        let remote = self
            .proxy
            .open_pipe_wire_remote(&self.session, Default::default())
            .await?;
        let size = stream.size();
        let description = size
            .map(|(w, h)| format!("Selected window · {w} × {h}"))
            .unwrap_or_else(|| "Selected window".into());
        Ok(Target {
            remote,
            node: stream.pipe_wire_node_id(),
            size,
            description,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub async fn close(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.session.close()).await;
    }
}

pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        matches!(
            e.downcast_ref::<ashpd::Error>(),
            Some(ashpd::Error::Response(
                ashpd::desktop::ResponseError::Cancelled
            ))
        )
    })
}
