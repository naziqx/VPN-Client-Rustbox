//! Background work. Long operations (network, starting/stopping the core) run on a
//! tokio runtime and report back to the UI thread through a channel.

use std::future::Future;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use rustbox_core::connection::Connection;
use rustbox_core::latency::ResultSink;
use rustbox_core::model::Latency;
use rustbox_core::subscription::FetchResult;
use rustbox_core::{GroupId, ProfileId};

pub enum TaskEvent {
    /// One latency result; tests report progressively.
    LatencyOne(ProfileId, Latency),
    /// A latency test identified by the id finished.
    TestFinished(u64),
    SubscriptionUpdated {
        group: GroupId,
        result: Result<FetchResult, String>,
    },
    Connected(Result<Connection, String>),
    Disconnected(Result<(), String>),
    TunGranted(Result<(), String>),
    /// Result of the request through a freshly started connection.
    Health(ProfileId, Result<u32, String>),
}

pub struct Tasks {
    runtime: tokio::runtime::Runtime,
    tx: Sender<TaskEvent>,
    rx: Receiver<TaskEvent>,
    ctx: egui::Context,
}

impl Tasks {
    pub fn new(ctx: egui::Context) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (tx, rx) = channel();
        Ok(Self {
            runtime,
            tx,
            rx,
            ctx,
        })
    }

    pub fn spawn(&self, fut: impl Future<Output = TaskEvent> + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    /// Like [`Self::spawn`], but the task can be cancelled with the returned handle.
    pub fn spawn_abortable(
        &self,
        fut: impl Future<Output = TaskEvent> + Send + 'static,
    ) -> tokio::task::AbortHandle {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime
            .spawn(async move {
                let _ = tx.send(fut.await);
                ctx.request_repaint();
            })
            .abort_handle()
    }

    /// Sink that forwards each latency result to the UI as it arrives.
    pub fn latency_sink(&self) -> ResultSink {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::sync::Arc::new(move |id, latency| {
            let _ = tx.send(TaskEvent::LatencyOne(id, latency));
            ctx.request_repaint();
        })
    }

    pub fn spawn_blocking(&self, f: impl FnOnce() -> TaskEvent + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime.spawn_blocking(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
    }

    pub fn poll(&self) -> Vec<TaskEvent> {
        self.rx.try_iter().collect()
    }
}
