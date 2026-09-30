//! Application state and actions. Rendering lives in `view.rs` and `dialogs.rs`.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use rustbox_core::connection::Connection;
use rustbox_core::latency::{self, UrlTestOptions};
use rustbox_core::model::{Latency, Profile};
use rustbox_core::process::{LogBuffer, core_version};
use rustbox_core::settings::TestKind;
use rustbox_core::storage::DEFAULT_GROUP;
use rustbox_core::{CoreKind, GroupId, ProfileId, Settings, Store, link, subscription};
use rustbox_platform::{Platform, TunPrivilege};

use crate::dialogs::Dialogs;
use crate::tasks::{TaskEvent, Tasks};

pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// A running latency test.
pub struct RunningTest {
    pub id: u64,
    pub handle: tokio::task::AbortHandle,
    pub pending: HashSet<ProfileId>,
    pub total: usize,
    pub ok: usize,
}

/// How traffic reaches the core. System proxy and TUN are separate settings,
/// but for the user they are mutually exclusive ways of working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only the local mixed port; apps are configured by hand.
    Proxy,
    /// The system proxy points at the mixed port.
    System,
    /// All system traffic through a TUN interface.
    Tun,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Proxy, Mode::System, Mode::Tun];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Proxy => "Прокси",
            Mode::System => "Системный прокси",
            Mode::Tun => "TUN",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Mode::Proxy => "Только локальный порт HTTP/SOCKS5: приложения настраиваются вручную",
            Mode::System => "Браузеры и большинство программ пойдут через прокси автоматически",
            Mode::Tun => "Весь трафик системы, включая игры и программы без поддержки прокси",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Type,
    Address,
    Name,
    Latency,
}

pub struct RustBoxApp {
    pub platform: Arc<dyn Platform>,
    pub settings: Settings,
    pub settings_path: PathBuf,
    pub store: Store,

    pub current_group: GroupId,
    pub selection: BTreeSet<ProfileId>,
    pub filter: String,

    pub connection: Option<Connection>,
    pub connecting: bool,
    /// When the current connection came up (for the uptime in the header).
    pub connected_at: Option<Instant>,
    pub logs: LogBuffer,
    pub show_logs: bool,
    /// Errors logged while the log panel was hidden (badge on the toggle).
    pub unseen_errors: usize,

    pub tasks: Tasks,
    pub test: Option<RunningTest>,
    next_test_id: u64,
    /// Table sort (column, ascending), like clicking a header in NekoBox.
    pub sort: Option<(SortKey, bool)>,
    pub updating: HashSet<GroupId>,

    pub status: Option<Status>,
    pub core_versions: Vec<(CoreKind, Result<String, String>)>,
    pub dialogs: Dialogs,
}

impl RustBoxApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let platform: Arc<dyn Platform> = Arc::from(rustbox_platform::current());
        let settings_path = Settings::path(platform.as_ref());
        let settings = Settings::load(&settings_path)?;
        let store = Store::load(&Store::path(platform.as_ref()))?;
        crate::theme::install(&cc.egui_ctx);

        let mut app = Self {
            tasks: Tasks::new(cc.egui_ctx.clone())?,
            platform,
            settings,
            settings_path,
            store,
            current_group: DEFAULT_GROUP,
            selection: BTreeSet::new(),
            filter: String::new(),
            connection: None,
            connecting: false,
            connected_at: None,
            logs: LogBuffer::default(),
            show_logs: false,
            unseen_errors: 0,
            test: None,
            next_test_id: 0,
            sort: None,
            updating: HashSet::new(),
            status: None,
            core_versions: Vec::new(),
            dialogs: Dialogs::default(),
        };
        if let Some(p) = app.settings.selected.and_then(|id| app.store.profile(id)) {
            app.current_group = p.group;
            app.selection.insert(p.id);
        }
        app.refresh_core_versions();
        app.logs.push(format!(
            "[rustbox] {} ready on {}",
            env!("CARGO_PKG_VERSION"),
            app.platform.name()
        ));
        Ok(app)
    }

    // ---------------------------------------------------------------- helpers

    pub fn info(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.logs.push(format!("[rustbox] {text}"));
        self.status = Some(Status {
            text,
            error: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.logs.push(format!("[rustbox] ERROR: {text}"));
        if !self.show_logs {
            self.unseen_errors += 1;
        }
        self.status = Some(Status {
            text,
            error: true,
            at: Instant::now(),
        });
    }

    pub fn save_store(&mut self) {
        if let Err(e) = self.store.save() {
            self.error(format!("не удалось сохранить профили: {e:#}"));
        }
    }

    pub fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.error(format!("не удалось сохранить настройки: {e:#}"));
        }
    }

    pub fn refresh_core_versions(&mut self) {
        self.core_versions = CoreKind::ALL
            .iter()
            .map(|&kind| {
                let v = self
                    .settings
                    .core_executable(kind, self.platform.as_ref())
                    .map_err(|e| format!("{e:#}"))
                    .map(|exe| {
                        core_version(kind, &exe).unwrap_or_else(|| exe.display().to_string())
                    });
                (kind, v)
            })
            .collect();
    }

    /// Profiles of the current group matching the search filter, in table order.
    pub fn visible_profiles(&self) -> Vec<&Profile> {
        let filter = self.filter.to_lowercase();
        let mut list: Vec<&Profile> = self
            .store
            .profiles_in(self.current_group)
            .filter(|p| {
                filter.is_empty()
                    || p.display_name().to_lowercase().contains(&filter)
                    || p.server.to_lowercase().contains(&filter)
                    || p.type_label().to_lowercase().contains(&filter)
            })
            .collect();
        if let Some((key, ascending)) = self.sort {
            match key {
                SortKey::Type => list.sort_by_key(|p| p.type_label()),
                SortKey::Address => list.sort_by_key(|p| (p.server.clone(), p.port)),
                SortKey::Name => list.sort_by_key(|p| p.display_name().to_lowercase()),
                // Like NekoBox: working servers by latency, then failures, then untested.
                SortKey::Latency => list.sort_by_key(|p| latency_rank(p.latency.as_ref())),
            }
            if !ascending {
                list.reverse();
            }
        }
        list
    }

    /// Click on a column header: sort ascending, then descending, then off.
    pub fn toggle_sort(&mut self, key: SortKey) {
        self.sort = match self.sort {
            Some((k, true)) if k == key => Some((key, false)),
            Some((k, false)) if k == key => None,
            _ => Some((key, true)),
        };
    }

    /// Selected profiles, or the whole visible list when nothing is selected.
    pub fn selected_or_visible(&self) -> Vec<Profile> {
        let visible = self.visible_profiles();
        let selected: Vec<Profile> = visible
            .iter()
            .filter(|p| self.selection.contains(&p.id))
            .map(|p| (*p).clone())
            .collect();
        if selected.len() > 1 {
            selected
        } else {
            visible.into_iter().cloned().collect()
        }
    }

    pub fn connected_profile(&self) -> Option<ProfileId> {
        self.connection.as_ref().map(|c| c.profile_id)
    }

    // ---------------------------------------------------------------- task results

    pub fn process_events(&mut self) {
        for event in self.tasks.poll() {
            match event {
                TaskEvent::LatencyOne(id, latency) => {
                    if let Some(test) = &mut self.test
                        && test.pending.remove(&id)
                        && matches!(latency, Latency::Ms(_))
                    {
                        test.ok += 1;
                    }
                    self.store.set_latency(id, latency);
                }
                TaskEvent::TestFinished(test_id) => {
                    if self.test.as_ref().is_some_and(|t| t.id == test_id) {
                        let test = self.test.take().unwrap();
                        self.save_store();
                        self.info(format!(
                            "тест завершён: {}/{} доступны",
                            test.ok, test.total
                        ));
                    }
                }
                TaskEvent::SubscriptionUpdated { group, result } => {
                    self.updating.remove(&group);
                    let name = self
                        .store
                        .group(group)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    match result {
                        Ok(r) => {
                            for e in &r.errors {
                                self.logs.push(format!("[subscription] пропущено: {e}"));
                            }
                            let count = self.store.replace_group_profiles(group, r.profiles);
                            if let Some(sub) = self
                                .store
                                .group_mut(group)
                                .and_then(|g| g.subscription.as_mut())
                            {
                                sub.info = r.info;
                                sub.last_updated = Some(unix_now());
                            }
                            self.save_store();
                            self.info(format!("подписка «{name}» обновлена: {count} профилей"));
                        }
                        Err(e) => self.error(format!("подписка «{name}»: {e}")),
                    }
                }
                TaskEvent::Connected(result) => {
                    self.connecting = false;
                    match result {
                        Ok(conn) => {
                            let name = self
                                .store
                                .profile(conn.profile_id)
                                .map(|p| p.display_name())
                                .unwrap_or_default();
                            let id = conn.profile_id;
                            self.connection = Some(conn);
                            self.connected_at = Some(Instant::now());
                            self.info(format!("подключено: {name}, проверяю интернет…"));
                            let settings = self.settings.clone();
                            self.tasks.spawn(async move {
                                let r = rustbox_core::connection::health_check(&settings)
                                    .await
                                    .map(|d| d.as_millis().max(1) as u32)
                                    .map_err(|e| format!("{e:#}"));
                                TaskEvent::Health(id, r)
                            });
                        }
                        Err(e) => self.error(e),
                    }
                }
                TaskEvent::Disconnected(result) => {
                    self.connecting = false;
                    self.connected_at = None;
                    match result {
                        Ok(()) => self.info("отключено"),
                        Err(e) => self.error(format!("отключено с ошибкой: {e}")),
                    }
                }
                TaskEvent::Health(id, result) => {
                    if self.connected_profile() != Some(id) {
                        continue;
                    }
                    match result {
                        Ok(ms) => {
                            self.store.set_latency(id, Latency::Ms(ms));
                            self.info(format!("✓ интернет через прокси работает ({ms} ms)"));
                        }
                        Err(e) => {
                            self.store.set_latency(id, Latency::Error(e.clone()));
                            self.error(format!(
                                "подключено, но интернет не работает: сервер не отвечает ({e}). \
                                 Выберите профиль с рабочим URL-тестом"
                            ));
                        }
                    }
                    self.save_store();
                }
                TaskEvent::TunGranted(result) => match result {
                    Ok(()) => {
                        self.info("права на TUN выданы");
                        self.dialogs.tun_hint = None;
                        self.set_tun(true);
                    }
                    Err(e) => self.error(format!("не удалось выдать права: {e}")),
                },
            }
        }

        // The core may die on its own (bad server, crash).
        if let Some(conn) = &mut self.connection
            && !conn.is_running()
        {
            let conn = self.connection.take().unwrap();
            let _ = conn.stop(self.platform.as_ref());
            self.connected_at = None;
            self.error("ядро неожиданно завершилось, смотрите лог");
        }
    }

    // ---------------------------------------------------------------- actions

    pub fn connect(&mut self, id: ProfileId) {
        if self.connecting {
            return;
        }
        let Some(profile) = self.store.profile(id).cloned() else {
            return;
        };
        if CoreKind::ALL
            .iter()
            .all(|c| c.check_profile(&profile).is_err())
        {
            if let Err(e) = self.settings.core.check_profile(&profile) {
                self.error(format!("{e:#}"));
            }
            return;
        }
        if self.settings.tun && !self.check_tun() {
            return;
        }
        if let Some(Latency::Error(e)) = &profile.latency {
            self.logs.push(format!(
                "[rustbox] WARN: последний тест этого профиля завершился ошибкой ({e})"
            ));
        }
        self.settings.selected = Some(id);
        self.save_settings();
        self.connecting = true;
        self.status = Some(Status {
            text: format!("подключение к {}…", profile.display_name()),
            error: false,
            at: Instant::now(),
        });

        let old = self.connection.take();
        let platform = self.platform.clone();
        let settings = self.settings.clone();
        let logs = self.logs.clone();
        self.tasks.spawn_blocking(move || {
            if let Some(old) = old {
                let _ = old.stop(platform.as_ref());
            }
            TaskEvent::Connected(
                Connection::start(platform.as_ref(), &settings, &profile, logs)
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    pub fn disconnect(&mut self) {
        let Some(conn) = self.connection.take() else {
            return;
        };
        self.connecting = true;
        let platform = self.platform.clone();
        self.tasks.spawn_blocking(move || {
            TaskEvent::Disconnected(conn.stop(platform.as_ref()).map_err(|e| format!("{e:#}")))
        });
    }

    /// Restarts the current connection to apply changed settings.
    pub fn reconnect(&mut self) {
        if let Some(id) = self.connected_profile() {
            self.connect(id);
        }
    }

    pub fn toggle_connection(&mut self) {
        if self.connection.is_some() {
            self.disconnect();
        } else if let Some(id) = self
            .selection
            .iter()
            .next()
            .copied()
            .or(self.settings.selected)
        {
            self.connect(id);
        } else {
            self.error("выберите профиль");
        }
    }

    /// Returns true when TUN may be used; otherwise opens the privilege dialog.
    /// TUN is always provided by sing-box (for Xray as a front, like Happ).
    pub fn check_tun(&mut self) -> bool {
        let exe = match self
            .settings
            .core_executable(CoreKind::SingBox, self.platform.as_ref())
        {
            Ok(exe) => exe,
            Err(e) => {
                self.error(format!("{e:#}"));
                return false;
            }
        };
        match self.platform.tun_privilege(&exe) {
            TunPrivilege::Granted => true,
            TunPrivilege::Missing { hint } => {
                self.dialogs.tun_hint = Some((exe, hint));
                false
            }
            TunPrivilege::Unsupported => {
                self.error("TUN не поддерживается на этой платформе");
                false
            }
        }
    }

    pub fn grant_tun(&mut self, exe: PathBuf) {
        let platform = self.platform.clone();
        self.tasks.spawn_blocking(move || {
            TaskEvent::TunGranted(
                platform
                    .grant_tun_privilege(&exe)
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    pub fn set_system_proxy(&mut self, enabled: bool) {
        self.settings.system_proxy = enabled;
        self.save_settings();
        let Some(conn) = &mut self.connection else {
            return;
        };
        let result = if enabled {
            self.platform
                .set_system_proxy(&self.settings.listen, self.settings.port)
        } else {
            self.platform.clear_system_proxy()
        };
        match result {
            Ok(()) => {
                conn.system_proxy = enabled;
                self.info(if enabled {
                    "системный прокси включён"
                } else {
                    "системный прокси выключен"
                });
            }
            Err(e) => self.error(format!("системный прокси: {e:#}")),
        }
    }

    pub fn set_tun(&mut self, enabled: bool) {
        if enabled && !self.check_tun() {
            return;
        }
        self.settings.tun = enabled;
        self.save_settings();
        self.reconnect();
    }

    pub fn mode(&self) -> Mode {
        if self.settings.tun {
            Mode::Tun
        } else if self.settings.system_proxy {
            Mode::System
        } else {
            Mode::Proxy
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode() {
            return;
        }
        match mode {
            Mode::Proxy => {
                if self.settings.system_proxy {
                    self.set_system_proxy(false);
                }
                if self.settings.tun {
                    self.set_tun(false);
                }
            }
            Mode::System => {
                if self.settings.tun {
                    // Leaving TUN restarts the core, which applies the system
                    // proxy from settings on start.
                    self.settings.system_proxy = true;
                    self.set_tun(false);
                } else {
                    self.set_system_proxy(true);
                }
            }
            Mode::Tun => {
                if !self.check_tun() {
                    return;
                }
                if self.settings.system_proxy {
                    self.set_system_proxy(false);
                }
                self.set_tun(true);
            }
        }
    }

    pub fn toggle_logs(&mut self) {
        self.show_logs = !self.show_logs;
        if self.show_logs {
            self.unseen_errors = 0;
        }
    }

    pub fn set_core(&mut self, core: CoreKind) {
        if self.settings.core == core {
            return;
        }
        self.settings.core = core;
        self.save_settings();
        self.info(format!("ядро: {core}"));
        self.reconnect();
    }

    pub fn import_text(&mut self, text: &str) {
        let (profiles, errors) = match subscription::parse_body(text) {
            Ok(r) => (r.profiles, r.errors),
            Err(_) => link::parse_many(text),
        };
        for e in &errors {
            self.logs.push(format!("[import] пропущено: {e}"));
        }
        if profiles.is_empty() {
            self.error("ссылки не найдены");
            return;
        }
        let group = match self.store.group(self.current_group) {
            Some(g) if g.subscription.is_none() => g.id,
            _ => DEFAULT_GROUP,
        };
        let ids = self.store.add_profiles(group, profiles);
        self.current_group = group;
        self.selection = ids.iter().copied().collect();
        self.save_store();
        self.info(format!(
            "импортировано профилей: {} (ошибок: {})",
            ids.len(),
            errors.len()
        ));
    }

    pub fn import_clipboard(&mut self) {
        match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => self.import_text(&text),
            Err(e) => self.error(format!("буфер обмена: {e}")),
        }
    }

    pub fn update_subscription(&mut self, group: GroupId) {
        let Some(sub) = self.store.group(group).and_then(|g| g.subscription.clone()) else {
            return;
        };
        if !self.updating.insert(group) {
            return;
        }
        let configured = sub
            .user_agent
            .clone()
            .unwrap_or_else(|| self.settings.subscription_user_agent.clone());
        let xray = self
            .settings
            .core_executable(CoreKind::Xray, self.platform.as_ref())
            .is_ok();
        let agents = subscription::user_agents(&configured, xray);
        let proxy = (self.settings.subscription_via_proxy && self.connection.is_some())
            .then(|| format!("http://{}:{}", self.settings.listen, self.settings.port));
        self.tasks.spawn(async move {
            let result = subscription::fetch_any(&sub.url, &agents, proxy.as_deref())
                .await
                .map_err(|e| format!("{e:#}"));
            TaskEvent::SubscriptionUpdated { group, result }
        });
    }

    pub fn update_all_subscriptions(&mut self) {
        let groups: Vec<GroupId> = self
            .store
            .groups
            .iter()
            .filter(|g| g.subscription.is_some())
            .map(|g| g.id)
            .collect();
        for g in groups {
            self.update_subscription(g);
        }
    }

    fn start_test(
        &mut self,
        profiles: &[Profile],
        run: impl FnOnce(
            latency::ResultSink,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    ) {
        self.stop_test();
        self.next_test_id += 1;
        let id = self.next_test_id;
        let sink = self.tasks.latency_sink();
        let fut = run(sink);
        let handle = self.tasks.spawn_abortable(async move {
            fut.await;
            TaskEvent::TestFinished(id)
        });
        self.test = Some(RunningTest {
            id,
            handle,
            pending: profiles.iter().map(|p| p.id).collect(),
            total: profiles.len(),
            ok: 0,
        });
    }

    /// Cancels the running test; results received so far are kept.
    pub fn stop_test(&mut self) {
        if let Some(test) = self.test.take() {
            test.handle.abort();
            self.save_store();
            self.info(format!(
                "тест остановлен: проверено {}/{}",
                test.total - test.pending.len(),
                test.total
            ));
        }
    }

    /// The "Проверить" button: the test chosen next to it, on the selected
    /// profiles or the whole visible list.
    pub fn run_test(&mut self) {
        let profiles = self.selected_or_visible();
        match self.settings.test_kind {
            TestKind::Url => self.url_test(profiles),
            TestKind::Tcp => self.tcp_ping(profiles),
        }
    }

    pub fn set_test_kind(&mut self, kind: TestKind) {
        if self.settings.test_kind != kind {
            self.settings.test_kind = kind;
            self.save_settings();
        }
    }

    pub fn tcp_ping(&mut self, profiles: Vec<Profile>) {
        if profiles.is_empty() {
            return;
        }
        let timeout = Duration::from_millis(self.settings.test_timeout_ms);
        let concurrency = self.settings.test_concurrency;
        self.info(format!("TCP ping: {} профилей…", profiles.len()));
        let list = profiles.clone();
        self.start_test(&profiles, move |sink| {
            Box::pin(latency::tcp_ping_many(list, timeout, concurrency, sink))
        });
    }

    pub fn url_test(&mut self, profiles: Vec<Profile>) {
        if profiles.is_empty() {
            return;
        }
        let core = self.settings.core;
        let exe = match self.settings.core_executable(core, self.platform.as_ref()) {
            Ok(exe) => exe,
            Err(e) => return self.error(format!("{e:#}")),
        };
        // Profiles sing-box cannot run are tested with Xray, as they would connect.
        let fallback = (core == CoreKind::SingBox)
            .then(|| {
                self.settings
                    .core_executable(CoreKind::Xray, self.platform.as_ref())
                    .ok()
            })
            .flatten();
        let work_dir = self.platform.runtime_dir();
        let env = self.platform.core_environment(false);
        let url = self.settings.test_url.clone();
        let timeout = Duration::from_millis(self.settings.test_timeout_ms);
        let concurrency = self.settings.test_concurrency;
        self.info(format!(
            "URL тест через {core}: {} профилей…",
            profiles.len()
        ));
        let list = profiles.clone();
        self.start_test(&profiles, move |sink| {
            Box::pin(async move {
                let opts = UrlTestOptions {
                    core,
                    exe: &exe,
                    fallback: fallback.as_deref().map(|p| (CoreKind::Xray, p)),
                    work_dir: &work_dir,
                    url: &url,
                    timeout,
                    concurrency,
                    env: &env,
                };
                latency::url_test_many(opts, list, sink).await;
            })
        });
    }

    pub fn delete_profiles(&mut self, ids: &[ProfileId]) {
        if ids.iter().any(|id| Some(*id) == self.connected_profile()) {
            self.disconnect();
        }
        self.store.remove_profiles(ids);
        self.selection.retain(|id| !ids.contains(id));
        self.save_store();
    }

    pub fn delete_group(&mut self, group: GroupId) {
        if let Some(id) = self.connected_profile()
            && self.store.profile(id).is_some_and(|p| p.group == group)
        {
            self.disconnect();
        }
        if self.store.remove_group(group) {
            self.current_group = DEFAULT_GROUP;
            self.selection.clear();
            self.save_store();
        }
    }

    pub fn copy_links(&mut self, ctx: &egui::Context, profiles: &[Profile]) {
        let text = profiles
            .iter()
            .map(link::to_link)
            .collect::<Vec<_>>()
            .join("\n");
        ctx.copy_text(text);
        self.info(format!("скопировано ссылок: {}", profiles.len()));
    }

    pub fn shutdown(&mut self) {
        if let Some(test) = self.test.take() {
            test.handle.abort();
        }
        if let Some(conn) = self.connection.take() {
            let _ = conn.stop(self.platform.as_ref());
        }
    }
}

impl eframe::App for RustBoxApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();
        if self.connection.is_some()
            || self.connecting
            || self.test.is_some()
            || !self.updating.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::view::show(self, ui);
        crate::dialogs::show(self, ui.ctx());
    }

    fn on_exit(&mut self) {
        self.shutdown();
    }
}

/// Sort order for latency: fastest first, failures after, untested last.
pub fn latency_rank(latency: Option<&Latency>) -> u64 {
    match latency {
        Some(Latency::Ms(ms)) => *ms as u64,
        Some(Latency::Error(_)) => u64::MAX - 1,
        None => u64::MAX,
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
