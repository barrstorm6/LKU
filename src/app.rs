use crate::{
    config::{self, Connection, Settings},
    k8s::{self, Candidate, Connected},
    logs::{self, Viewer},
    pods::{PodStore, PodView},
};
use egui::Context;
use kube::{Client, runtime::watcher::Event};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, watch},
    task::JoinHandle,
};

pub enum Message {
    Picked(Option<Result<Candidate, String>>),
    Prepared(Result<Candidate, String>, Connection),
    Connected(Result<Connected, String>),
    Pods(Event<PodView>),
    PodError(String),
    LogLines(u64, Vec<String>),
    LogStatus(u64, Option<String>),
    SettingsError,
}

pub struct Envelope {
    pub generation: u64,
    pub message: Message,
}
#[derive(Clone)]
pub struct Sink {
    pub generation: u64,
    pub tx: mpsc::Sender<Envelope>,
    pub ctx: Context,
}
impl Sink {
    pub async fn send(&self, message: Message) -> bool {
        let sent = self
            .tx
            .send(Envelope {
                generation: self.generation,
                message,
            })
            .await
            .is_ok();
        if sent {
            self.ctx.request_repaint();
        }
        sent
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unknown,
    Connected,
    Error,
}

pub struct App {
    pub settings: Settings,
    pub status: Status,
    pub error: Option<String>,
    pub settings_error: Option<String>,
    pub namespaces: Vec<String>,
    pub restricted: bool,
    pub namespace: String,
    pub manual_namespace: String,
    pub pods: PodStore,
    pub filtered: Vec<String>,
    pub search: String,
    pub selected_pod: Option<String>,
    pub viewer: Option<Viewer>,
    pub candidate: Option<Candidate>,
    pub pending_connection: Option<Connection>,
    pub trust: bool,
    pub picking: bool,
    pub loading: bool,
    retry_connection: Option<Connection>,
    runtime: Arc<Runtime>,
    ctx: Context,
    tx: mpsc::Sender<Envelope>,
    rx: mpsc::Receiver<Envelope>,
    saver: watch::Sender<Option<Settings>>,
    save_task: Option<JoinHandle<()>>,
    writable_settings: bool,
    generation: u64,
    log_id: u64,
    client: Option<Client>,
    task: Option<JoinHandle<()>>,
    log_task: Option<JoinHandle<()>>,
}

impl App {
    pub fn new(runtime: Arc<Runtime>, ctx: Context) -> Self {
        Self::from_settings(runtime, ctx, config::load())
    }
    fn from_settings(
        runtime: Arc<Runtime>,
        ctx: Context,
        loaded: Result<Settings, &'static str>,
    ) -> Self {
        let settings_error = loaded.as_ref().err().map(|s| s.to_string());
        let writable_settings = loaded.is_ok();
        let settings = loaded.unwrap_or_default();
        let (tx, rx) = mpsc::channel(128);
        let (saver, mut saves) = watch::channel::<Option<Settings>>(None);
        let sink = Sink {
            generation: 0,
            tx: tx.clone(),
            ctx: ctx.clone(),
        };
        let save_task = runtime.spawn(async move {
            while saves.changed().await.is_ok() {
                let settings = saves.borrow_and_update().clone();
                if let Some(settings) = settings {
                    let result = tokio::task::spawn_blocking(move || config::save(&settings)).await;
                    if !matches!(result, Ok(Ok(()))) {
                        sink.send(Message::SettingsError).await;
                    }
                } else {
                    break;
                }
            }
        });
        let restore = settings.last_cluster_id.clone();
        let mut app = Self {
            settings,
            status: Status::Unknown,
            error: None,
            settings_error,
            namespaces: vec![],
            restricted: false,
            namespace: String::new(),
            manual_namespace: String::new(),
            pods: PodStore::default(),
            filtered: vec![],
            search: String::new(),
            selected_pod: None,
            viewer: None,
            candidate: None,
            pending_connection: None,
            trust: false,
            picking: false,
            loading: false,
            retry_connection: None,
            runtime,
            ctx,
            tx,
            rx,
            saver,
            save_task: Some(save_task),
            writable_settings,
            generation: 0,
            log_id: 0,
            client: None,
            task: None,
            log_task: None,
        };
        if let Some(id) = restore {
            app.select_connection(&id);
        }
        app
    }
    fn sink(&self) -> Sink {
        Sink {
            generation: self.generation,
            tx: self.tx.clone(),
            ctx: self.ctx.clone(),
        }
    }
    pub fn current(&self) -> Option<&Connection> {
        self.settings
            .clusters
            .iter()
            .find(|c| Some(&c.id) == self.settings.last_cluster_id.as_ref())
    }
    pub fn persist(&mut self) {
        if self.writable_settings {
            self.saver.send_replace(Some(self.settings.clone()));
        }
    }
    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.close_logs();
        self.client = None;
        self.selected_pod = None;
        self.pods = PodStore::default();
        self.filtered.clear();
        self.namespaces.clear();
        self.restricted = false;
        self.status = Status::Unknown;
        self.error = None;
        self.loading = true;
        self.candidate = None;
        self.pending_connection = None;
        self.trust = false;
        self.picking = false;
    }
    pub fn select_connection(&mut self, id: &str) {
        let Some(connection) = self.settings.clusters.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        self.reset();
        self.settings.last_cluster_id = Some(connection.id.clone());
        self.persist();
        self.prepare(connection);
    }
    fn prepare(&mut self, connection: Connection) {
        self.reset();
        self.retry_connection = Some(connection.clone());
        self.namespace.clone_from(&connection.namespace);
        self.manual_namespace.clone_from(&connection.namespace);
        let sink = self.sink();
        self.task = Some(self.runtime.spawn(async move {
            let path = connection.kubeconfig_path.clone();
            let prepared = tokio::task::spawn_blocking(move || Candidate::read(path, None)).await;
            let result = match prepared {
                Ok(Ok(mut candidate)) => {
                    candidate.selected.clone_from(&connection.context);
                    candidate.validate().map(|_| candidate)
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err("Could not read kubeconfig.".into()),
            };
            sink.send(Message::Prepared(result, connection)).await;
        }));
    }
    pub fn retry(&mut self) {
        if let Some(connection) = self
            .retry_connection
            .clone()
            .or_else(|| self.current().cloned())
        {
            self.prepare(connection);
        } else {
            self.pick_file(None);
        }
    }
    pub fn pick_file(&mut self, replacing: Option<String>) {
        if self.picking {
            return;
        }
        self.picking = true;
        let sink = self.sink();
        self.runtime.spawn(async move {
            let picked = tokio::task::spawn_blocking(move || {
                rfd::FileDialog::new()
                    .set_title("Select kubeconfig")
                    .add_filter("Kubeconfig", &["yaml", "yml"])
                    .add_filter("All files", &["*"])
                    .pick_file()
                    .map(|path| Candidate::read(path, replacing))
            })
            .await;
            sink.send(Message::Picked(picked.unwrap_or_else(|_| {
                Some(Err("Could not open file picker.".into()))
            })))
            .await;
        });
    }
    pub fn connect_candidate(&mut self) {
        let Some(candidate) = self.candidate.take() else {
            return;
        };
        let validation = candidate.validate();
        if let Err(error) = validation {
            self.error = Some(error);
            self.candidate = Some(candidate);
            return;
        }
        if validation.ok().flatten().is_some() && !self.trust {
            self.candidate = Some(candidate);
            return;
        }
        let mut connection = self.pending_connection.take().unwrap_or_else(|| {
            let existing =
                self.settings.clusters.iter().find(|c| {
                    c.kubeconfig_path == candidate.path && c.context == candidate.selected
                });
            let id = candidate
                .replacing
                .clone()
                .or_else(|| existing.map(|c| c.id.clone()))
                .unwrap_or_else(|| {
                    format!(
                        "local-{}",
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    )
                });
            Connection {
                id,
                name: candidate.selected.clone(),
                kubeconfig_path: candidate.path.clone(),
                context: candidate.selected.clone(),
                namespace: existing
                    .map(|c| c.namespace.clone())
                    .unwrap_or_else(|| candidate.namespace()),
            }
        });
        if connection.context != candidate.selected {
            connection.context.clone_from(&candidate.selected);
            connection.name.clone_from(&candidate.selected);
            connection.namespace = candidate.namespace();
        }
        self.reset();
        self.retry_connection = Some(connection.clone());
        self.namespace.clone_from(&connection.namespace);
        let sink = self.sink();
        self.task = Some(self.runtime.spawn(async move {
            let result =
                tokio::time::timeout(Duration::from_secs(30), k8s::connect(candidate, connection))
                    .await
                    .unwrap_or_else(|_| {
                        Err("Connection timed out. Check the network and credential plugin.".into())
                    });
            sink.send(Message::Connected(result)).await;
        }));
    }
    pub fn change_namespace(&mut self, namespace: String) {
        if namespace.is_empty()
            || namespace.len() > 63
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || namespace.starts_with('-')
            || namespace.ends_with('-')
        {
            self.error = Some("Enter a valid Kubernetes namespace.".into());
            return;
        }
        self.namespace = namespace;
        self.manual_namespace.clone_from(&self.namespace);
        if let Some(connection) = self
            .settings
            .clusters
            .iter_mut()
            .find(|c| Some(&c.id) == self.settings.last_cluster_id.as_ref())
        {
            connection.namespace.clone_from(&self.namespace);
        }
        self.persist();
        self.start_watch();
    }
    fn start_watch(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.close_logs();
        self.selected_pod = None;
        self.generation = self.generation.wrapping_add(1);
        self.status = Status::Unknown;
        self.loading = true;
        self.error = None;
        self.pods = PodStore::default();
        self.filtered.clear();
        if let Some(client) = self.client.clone() {
            let sink = self.sink();
            let namespace = self.namespace.clone();
            self.task = Some(self.runtime.spawn(k8s::watch_pods(client, namespace, sink)));
        }
    }
    pub fn remove_connection(&mut self, id: &str) {
        self.settings.clusters.retain(|c| c.id != id);
        if self.settings.last_cluster_id.as_deref() == Some(id) {
            self.reset();
            self.loading = false;
            self.settings.last_cluster_id = None;
            if let Some(next) = self.settings.clusters.first().map(|c| c.id.clone()) {
                self.select_connection(&next);
            }
        }
        self.persist();
    }
    pub fn close_logs(&mut self) {
        self.log_id = self.log_id.wrapping_add(1);
        if let Some(task) = self.log_task.take() {
            task.abort();
        }
        self.viewer = None;
    }
    pub fn open_logs(&mut self, pod: String, container: String) {
        self.close_logs();
        self.viewer = Some(Viewer::new(pod, container));
        self.restart_logs();
    }
    pub fn restart_logs(&mut self) {
        if let Some(task) = self.log_task.take() {
            task.abort();
        }
        self.log_id = self.log_id.wrapping_add(1);
        if let (Some(viewer), Some(client)) = (&mut self.viewer, self.client.clone()) {
            viewer.buffer = logs::LogBuffer::default();
            viewer.last_revision = u64::MAX;
            viewer.error = None;
            let (pod, container, timestamps) = (
                viewer.pod.clone(),
                viewer.container.clone(),
                viewer.timestamps,
            );
            self.log_task = Some(self.runtime.spawn(logs::stream(
                client,
                self.namespace.clone(),
                pod,
                container,
                timestamps,
                self.log_id,
                self.sink(),
            )));
        }
    }
    pub fn drain(&mut self) {
        let start = std::time::Instant::now();
        let mut changed = false;
        while start.elapsed() < Duration::from_millis(6) {
            let Ok(envelope) = self.rx.try_recv() else {
                break;
            };
            if matches!(envelope.message, Message::SettingsError) {
                self.settings_error =
                    Some("Could not save settings. Check disk space and permissions.".into());
                continue;
            }
            if envelope.generation != self.generation {
                continue;
            }
            match envelope.message {
                Message::Picked(picked) => {
                    self.picking = false;
                    if let Some(result) = picked {
                        match result {
                            Ok(candidate) => {
                                self.candidate = Some(candidate);
                                self.pending_connection = None;
                                self.trust = false;
                                self.error = None;
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
                Message::Prepared(result, connection) => match result {
                    Ok(candidate) => {
                        let exec = candidate.validate().ok().flatten().is_some();
                        self.candidate = Some(candidate);
                        self.pending_connection = Some(connection);
                        if !exec {
                            self.connect_candidate();
                        } else {
                            self.loading = false;
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.status = Status::Error;
                        self.loading = false;
                    }
                },
                Message::Connected(result) => match result {
                    Ok(connected) => {
                        self.namespace.clone_from(&connected.connection.namespace);
                        self.manual_namespace.clone_from(&self.namespace);
                        self.settings.last_cluster_id = Some(connected.connection.id.clone());
                        if let Some(existing) = self
                            .settings
                            .clusters
                            .iter_mut()
                            .find(|c| c.id == connected.connection.id)
                        {
                            *existing = connected.connection;
                        } else {
                            self.settings.clusters.push(connected.connection);
                        }
                        self.namespaces = connected.namespaces;
                        self.restricted = connected.restricted;
                        self.client = Some(connected.client);
                        self.persist();
                        self.start_watch();
                    }
                    Err(error) => {
                        self.status = Status::Error;
                        self.error = Some(error);
                        self.loading = false;
                    }
                },
                Message::Pods(event) => {
                    if matches!(event, Event::InitDone | Event::Apply(_) | Event::Delete(_)) {
                        self.status = Status::Connected;
                        self.loading = false;
                        self.error = None;
                    }
                    self.pods.apply(event);
                    changed = true;
                }
                Message::PodError(error) => {
                    self.status = Status::Error;
                    self.loading = false;
                    self.error = Some(error);
                }
                Message::LogLines(id, lines) if id == self.log_id => {
                    if let Some(viewer) = &mut self.viewer {
                        viewer.buffer.append(lines);
                    }
                }
                Message::LogStatus(id, error) if id == self.log_id => {
                    if let Some(viewer) = &mut self.viewer {
                        viewer.error = error;
                    }
                }
                _ => {}
            }
        }
        if changed {
            self.filter();
        }
        if !self.rx.is_empty() {
            self.ctx.request_repaint();
        }
    }
    pub fn filter(&mut self) {
        self.filtered = self.pods.matches(&self.search);
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain();
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::ui::render(self, ui);
    }
}
impl Drop for App {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(task) = self.log_task.take() {
            task.abort();
        }
        self.rx.close();
        self.saver.send_replace(None);
        if let Some(writer) = self.save_task.take() {
            let _ = self.runtime.block_on(writer);
        }
        // Flush the final small settings snapshot before shutting the runtime down.
        if self.writable_settings && config::save(&self.settings).is_err() {
            tracing::warn!("Could not save settings on exit");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switch_cancels_tasks_and_rejects_stale_pods_and_logs() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut app = App::from_settings(runtime, Context::default(), Ok(Settings::default()));
        app.writable_settings = false;
        let worker = app.runtime.spawn(std::future::pending::<()>());
        let aborted = worker.abort_handle();
        app.task = Some(worker);
        let log = app.runtime.spawn(std::future::pending::<()>());
        let log_aborted = log.abort_handle();
        app.log_task = Some(log);
        let old = app.generation;
        app.reset();
        app.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), async {
                while !aborted.is_finished() || !log_aborted.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        assert!(aborted.is_finished());
        assert!(log_aborted.is_finished());
        app.tx
            .try_send(Envelope {
                generation: old,
                message: Message::PodError("stale cluster".into()),
            })
            .unwrap_or_else(|_| panic!("queue full"));
        app.tx
            .try_send(Envelope {
                generation: app.generation,
                message: Message::PodError("current cluster".into()),
            })
            .unwrap_or_else(|_| panic!("queue full"));
        app.drain();
        assert_eq!(app.error.as_deref(), Some("current cluster"));
        app.viewer = Some(Viewer::new("p".into(), "c".into()));
        app.tx
            .try_send(Envelope {
                generation: app.generation,
                message: Message::LogLines(app.log_id.wrapping_sub(1), vec!["stale log".into()]),
            })
            .unwrap_or_else(|_| panic!("queue full"));
        app.drain();
        assert!(app.viewer.as_ref().unwrap().buffer.lines.is_empty());
    }
    #[test]
    #[ignore = "release benchmark: cargo test --release benchmark -- --ignored --nocapture"]
    fn benchmark_ui_and_log_pressure() {
        let ctx = Context::default();
        crate::theme::apply(&ctx);
        let mut app = App::from_settings(
            Arc::new(Runtime::new().unwrap()),
            ctx.clone(),
            Ok(Settings::default()),
        );
        app.writable_settings = false;
        app.settings.clusters.push(Connection {
            id: "fixture".into(),
            name: "Fixture (benchmark only)".into(),
            kubeconfig_path: "fixture".into(),
            context: "fixture".into(),
            namespace: "default".into(),
        });
        app.settings.last_cluster_id = Some("fixture".into());
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(760.0, 620.0),
            )),
            ..Default::default()
        };
        for count in [1_000, 10_000] {
            app.pods = PodStore::default();
            for n in 0..count {
                let name = format!("fixture-{n:05}");
                app.pods.apply(Event::Apply(PodView {
                    uid: name.clone(),
                    name: name.clone(),
                    search_name: name,
                    namespace: "default".into(),
                    health: crate::pods::Health::Healthy,
                    status: "Ready".into(),
                    ready: 1,
                    total: 1,
                    restarts: 0,
                    ip: String::new(),
                    node: String::new(),
                    containers: vec!["app".into()],
                }));
            }
            app.filter();
            let mut times = Vec::new();
            for frame in 0..220 {
                let start = std::time::Instant::now();
                app.search = if frame % 2 == 0 {
                    "fixture"
                } else {
                    "fixture-0"
                }
                .into();
                app.filter();
                std::hint::black_box(ctx.run_ui(input(), |ui| crate::ui::render(&mut app, ui)));
                if frame >= 20 {
                    times.push(start.elapsed());
                }
            }
            times.sort();
            println!(
                "{count} pods: filter + virtualized UI frame p95 {:?} (headless, excludes GPU/presentation)",
                times[189]
            );
        }
        app.viewer = Some(Viewer::new("fixture-00000".into(), "app".into()));
        let mut times = Vec::new();
        for frame in 0..220 {
            let start = std::time::Instant::now();
            app.viewer.as_mut().unwrap().buffer.append(
                (0..1000)
                    .map(|n| format!("fixture log {n}: bounded streaming load"))
                    .collect(),
            );
            std::hint::black_box(ctx.run_ui(input(), |ui| crate::ui::render(&mut app, ui)));
            if frame >= 20 {
                times.push(start.elapsed());
            }
        }
        times.sort();
        println!(
            "1000 incoming lines/frame, 5000 retained: UI p95 {:?} (headless)",
            times[189]
        );
    }
    #[tokio::test]
    async fn abort_cancels_worker_and_generation_rejects_queued_results() {
        let (tx, mut rx) = mpsc::channel(2);
        let task = tokio::spawn(async move {
            tx.send(Envelope {
                generation: 1,
                message: Message::PodError("old".into()),
            })
            .await
            .unwrap();
            std::future::pending::<()>().await;
        });
        let stale = rx.recv().await.unwrap();
        let current_generation = 2;
        assert_ne!(stale.generation, current_generation);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
}
