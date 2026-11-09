#![windows_subsystem = "windows"]

mod help;
mod i18n;
mod icons;
mod lan_sync;
mod scrolling;
mod ui;
#[cfg(windows)]
fn tray_pixels(logo: &[u8], state: u8) -> Vec<u8> {
    let tint = match state {
        1 => [111.0, 230.0, 180.0],
        2 => [241.0, 194.0, 92.0],
        _ => [145.0, 151.0, 160.0],
    };
    let mut pixels = logo.to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        let light =
            (0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32)
                / 255.0;
        for channel in 0..3 {
            pixel[channel] = (tint[channel] * (0.45 + 0.55 * light)) as u8;
        }
    }
    pixels
}
use everty_ui::*;
use multilink_core::{ImportReport, Settings};
use multilink_mihomo::{Engine, Metrics};
use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, Ime, MouseButton as WMouse, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key as WKey, ModifiersState, NamedKey},
    window::Window,
};

enum Command {
    Activate(multilink_core::PendingActivation, Settings),
    RefreshAccess(multilink_core::ClientAccess),
    Connect(Settings),
    Disconnect,
    Import(String, Option<u64>),
    Save(Settings),
    SaveForRestart(Settings, mpsc::Sender<Result<(), String>>),
    ApplyRouting(Vec<String>, multilink_core::Policy),
    ApplySettings(Settings),
    Quit,
}
enum Update {
    AccessSynced(String, multilink_core::ProfileDocument),
    AccessChecked(u64),
    AccessReady(String, multilink_core::ProfileDocument),
    AccessRejected,
    Connected(bool),
    Disconnected,
    Error(String),
    Imported(ImportReport, Option<String>, Option<u64>),
    SubscriptionFetched(u64, u64, String, Result<ImportReport, String>),
    AntigravityChecked(String, Result<multilink_mihomo::AntigravityCheck, String>),
    SaveError,
    Notice(String),
    OperationError(String),
    RoutingApplied(
        Vec<String>,
        multilink_core::Policy,
        multilink_mihomo::RoutingChange,
    ),
    RoutingFailed(String),
    Metrics(
        Metrics,
        Vec<(String, Option<u64>, multilink_core::NodeQuality)>,
    ),
    TunnelReachable(bool),
    UpdateCheckResult(Result<Option<(String, String)>, String>),
}
#[derive(Clone)]
struct UiUpdates {
    channel: mpsc::Sender<Update>,
    wake: Option<EventLoopProxy<()>>,
}
impl UiUpdates {
    fn send(&self, update: Update) -> Result<(), mpsc::SendError<Update>> {
        self.channel.send(update)?;
        if let Some(wake) = &self.wake {
            let _ = wake.send_event(());
        }
        Ok(())
    }
}
// Declaration order is intentional: unwind restores the proxy before killing
// the core, even if the worker panics outside the normal stop path.
struct Connection {
    access: Option<multilink_core::ClientAccess>,
    proxy: Option<multilink_platform::SystemProxyGuard>,
    engine: Engine,
}
impl Connection {
    fn start(settings: &Settings) -> Result<Self, String> {
        if settings.tun
            && !multilink_platform::is_elevated()
                .map_err(|e| format!("Не удалось проверить права Windows: {e}"))?
        {
            return Err("Для TUN полностью закройте приложение через трей и запустите его от имени администратора".into());
        }
        let engine = Engine::start(settings)?;
        let proxy = if settings.system_proxy {
            Some(
                multilink_platform::SystemProxyGuard::enable(multilink_mihomo::PORT)
                    .map_err(|e| format!("Не удалось включить системный прокси: {e}"))?,
            )
        } else {
            None
        };
        Ok(Self {
            proxy,
            engine,
            access: settings
                .access
                .as_ref()
                .filter(|a| {
                    settings
                        .nodes
                        .iter()
                        .any(|n| n.enabled && n.subscription_ids.contains(&a.subscription_id))
                })
                .cloned(),
        })
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(proxy) = self.proxy.as_mut() {
            proxy
                .restore()
                .map_err(|e| format!("Не удалось восстановить системный прокси: {e}"))?;
        }
        self.proxy = None;
        Ok(())
    }
}
fn worker(rx: mpsc::Receiver<Command>, tx: UiUpdates) {
    let mut connection: Option<Connection> = None;
    let mut next_tunnel_check = Instant::now();
    let mut next_access_check = Instant::now();
    let mut tunnel_state: Option<bool> = None;
    let mut metrics_failures: u32 = 0;
    match multilink_platform::recover_system_proxy() {
        Ok(true) => {
            let _ = tx.send(Update::Notice(
                "Исходный системный прокси восстановлен после предыдущего сбоя".into(),
            ));
        }
        Err(e) => {
            let _ = tx.send(Update::OperationError(format!(
                "Ошибка восстановления системного прокси: {e}"
            )));
        }
        Ok(false) => {}
    }
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Command::Activate(pending, settings)) => {
                let saved = serde_json::to_vec(&settings)
                    .ok()
                    .and_then(|bytes| multilink_platform::save(&bytes).ok())
                    .is_some();
                if !saved {
                    let _ = tx.send(Update::OperationError(
                        "Не удалось сохранить активацию. Запрос на сервер не отправлен".into(),
                    ));
                    continue;
                }
                let update = match multilink_mihomo::activate_access(&pending) {
                    Ok((token, doc)) => Update::AccessReady(token, doc),
                    Err(multilink_mihomo::AccessError::Denied) => Update::OperationError(
                        "Ключ не найден, уже использован или срок активации истёк".into(),
                    ),
                    Err(multilink_mihomo::AccessError::Message(message)) => {
                        Update::OperationError(message)
                    }
                };
                let _ = tx.send(update);
            }
            Ok(Command::RefreshAccess(access)) => {
                let update = match multilink_mihomo::refresh_access(&access) {
                    Ok((token, doc)) => Update::AccessReady(token, doc),
                    Err(multilink_mihomo::AccessError::Denied) => Update::AccessRejected,
                    Err(multilink_mihomo::AccessError::Message(message)) => {
                        Update::OperationError(message)
                    }
                };
                let _ = tx.send(update);
            }
            Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Connect(mut settings)) => {
                if let Some(access) = settings
                    .access
                    .as_ref()
                    .filter(|a| {
                        settings
                            .nodes
                            .iter()
                            .any(|n| n.enabled && n.subscription_ids.contains(&a.subscription_id))
                    })
                    .cloned()
                {
                    match multilink_mihomo::refresh_access(&access) {
                        Ok((token, doc)) => {
                            if let Err(error) = multilink_core::install_access(
                                &mut settings,
                                token.clone(),
                                doc.clone(),
                            ) {
                                let _ = tx.send(Update::OperationError(error));
                                continue;
                            }
                            let _ = tx.send(Update::AccessSynced(token, doc));
                        }
                        Err(multilink_mihomo::AccessError::Denied) => {
                            let _ = tx.send(Update::AccessRejected);
                            continue;
                        }
                        Err(multilink_mihomo::AccessError::Message(message)) => {
                            let _ = tx.send(Update::OperationError(message));
                            continue;
                        }
                    }
                }
                next_access_check = Instant::now() + Duration::from_secs(600);
                match Connection::start(&settings) {
                    Ok(e) => {
                        let system_proxy_active = e.proxy.is_some();
                        connection = Some(e);
                        metrics_failures = 0;
                        multilink_platform::prevent_sleep(true);
                        next_tunnel_check = Instant::now();
                        tunnel_state = None;
                        let _ = tx.send(Update::Connected(system_proxy_active));
                        let missing = multilink_core::unavailable_route_targets(&settings);
                        if !missing.is_empty() {
                            let _ = tx.send(Update::Notice(format!(
                                "Недоступных привязок: {} · временно используется общий VPN-пул",
                                missing.len()
                            )));
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Update::Error(error));
                    }
                }
            }
            Ok(Command::Disconnect) => {
                let result = connection.as_mut().map(Connection::stop).unwrap_or(Ok(()));
                match result {
                    Ok(()) => {
                        connection = None;
                        tunnel_state = None;
                        metrics_failures = 0;
                        multilink_platform::prevent_sleep(false);
                        let _ = tx.send(Update::Disconnected);
                    }
                    Err(e) => {
                        let _ = tx.send(Update::OperationError(format!(
                            "{e}. Ядро оставлено запущенным; повторите отключение."
                        )));
                    }
                }
            }
            Ok(Command::Import(input, source)) => {
                let url = input
                    .trim()
                    .starts_with("https://")
                    .then(|| input.trim().to_owned());
                let result = if let Some(url) = &url {
                    multilink_core::subscription_url(url).and_then(|_| {
                        multilink_mihomo::fetch_subscription(input.trim())
                            .and_then(|s| multilink_core::import_subscription(&s))
                    })
                } else {
                    multilink_core::import_subscription(&input)
                };
                let update = match result {
                    Ok(nodes) => Update::Imported(nodes, url, source),
                    Err(e) => Update::OperationError(e),
                };
                let _ = tx.send(update);
            }
            Ok(Command::Save(settings)) => {
                if serde_json::to_vec(&settings)
                    .ok()
                    .and_then(|bytes| multilink_platform::save(&bytes).ok())
                    .is_none()
                {
                    let _ = tx.send(Update::SaveError);
                }
            }
            Ok(Command::SaveForRestart(settings, reply)) => {
                let result = serde_json::to_vec(&settings)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| multilink_platform::save(&bytes).map_err(|e| e.to_string()));
                let _ = reply.send(result);
            }
            Ok(Command::ApplyRouting(rules, policy)) => {
                let result = connection
                    .as_mut()
                    .ok_or_else(|| "Подключение уже остановлено".to_owned())
                    .and_then(|c| c.engine.apply_routing(&rules, policy));
                let update = match result {
                    Ok(change) => Update::RoutingApplied(rules, policy, change),
                    Err(error) => Update::RoutingFailed(error),
                };
                let _ = tx.send(update);
            }
            Ok(Command::ApplySettings(settings)) => {
                if let Some(c) = connection.as_mut() {
                    if let Err(error) = c.engine.apply_settings(&settings) {
                        let _ = tx.send(Update::OperationError(error));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= next_access_check {
            next_access_check = Instant::now() + Duration::from_secs(600);
            if let Some(access) = connection.as_ref().and_then(|c| c.access.clone()) {
                match multilink_mihomo::refresh_access(&access) {
                    Ok((_, doc)) => {
                        if let Some(expiry) = doc.expires_at {
                            if let Some(c) = connection.as_mut() {
                                if let Some(a) = &mut c.access {
                                    a.expires_at = expiry;
                                }
                            }
                            let _ = tx.send(Update::AccessChecked(expiry));
                        }
                    }
                    Err(multilink_mihomo::AccessError::Denied) => {
                        multilink_platform::prevent_sleep(false);
                        if let Some(mut c) = connection.take() {
                            let _ = c.stop();
                        }
                        metrics_failures = 0;
                        let _ = tx.send(Update::Disconnected);
                        let _ = tx.send(Update::AccessRejected);
                        continue;
                    }
                    Err(multilink_mihomo::AccessError::Message(_)) => {}
                }
            }
        }
        if let Some(c) = connection.as_mut() {
            if Instant::now() >= next_tunnel_check {
                next_tunnel_check = Instant::now() + Duration::from_secs(8);
                let reachable = c.engine.tunnel_reachable();
                if tunnel_state != Some(reachable) {
                    tunnel_state = Some(reachable);
                    let _ = tx.send(Update::TunnelReachable(reachable));
                }
            }
            match c.engine.metrics() {
                Ok(metrics) => {
                    metrics_failures = 0;
                    let _ = tx.send(Update::Metrics(
                        metrics,
                        c.engine.delays().unwrap_or_default(),
                    ));
                }
                Err(error) => {
                    let child_alive = c.engine.is_alive();
                    if child_alive && metrics_failures < 5 {
                        metrics_failures += 1;
                        // Transient loopback/sleep wake-up glitch. Mihomo process is alive,
                        // do not disconnect, keep VPN active and retry next second.
                    } else {
                        metrics_failures = 0;
                        multilink_platform::prevent_sleep(false);
                        let automatic = c.proxy.is_some();
                        let restore = c.stop();
                        connection = None;
                        let message = match restore {
                            Ok(()) if automatic => format!("{error} · системный прокси восстановлен"),
                            Ok(()) => error,
                            Err(e) => format!("{error}. {e}; восстановление повторит watchdog"),
                        };
                        let _ = tx.send(Update::Error(message));
                    }
                }
            }
        }
    }
    multilink_platform::prevent_sleep(false);
    if let Some(mut connection) = connection {
        let _ = connection.stop();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Dashboard,
    Access,
    Servers,
    Subscriptions,
    Routing,
    Statistics,
    Settings,
    About,
    Help,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Language,
    SelectLanguage(usize),
    HelpSection(usize),
    ActivateAccess,
    RefreshAccess,
    Nav(Page),
    Connect,
    Import,
    Close,
    Submit,
    Paste,
    Policy,
    Toggle(usize),
    EnableAllServers,
    DisableAllServers,
    RankServers,
    DeleteRule(usize),
    EditRule(usize),
    RuleNote,
    ImportRoutes,
    ExportRoutes,
    ReplaceRoutes,
    ExportSettings,
    ImportSettings,
    AddRule,
    AddProcessRule,
    AddAntigravity,
    CheckAntigravity,
    RuleField(bool),
    AddRuDomains,
    ClearServers,
    ToggleSystemProxy,
    ToggleTun,
    Compact,
    Expanded,
    ToggleAutostart,
    RuleTarget,
    SelectRuleTarget(usize),
    TargetPage(usize),
    OpenAboutSite,
    OpenGitHub,
    OpenDonate,
    CopyQrContent,
    ShareNode(usize),
    ShareSubscription(u64),
    OpenLanSync,
    RefreshSubscription(u64),
    EditSubscriptionUrl(u64),
    RenameSubscription(u64),
    DeleteSubscription(u64),
    ToggleAutoconnect,
    ToggleRulesOnly,
    OpenSetPin,
    RemovePin,
    LockPin,
    CheckUpdate,
    OpenUpdateLink,
}
#[derive(Clone, PartialEq, Debug)]
enum Modal {
    Language,
    AccessCode,
    Import,
    Rule,
    RuRule,
    ProcessRule,
    Note,
    RouteImport,
    SubscriptionUrl,
    SubscriptionName,
    SubscriptionDelete,
    ClearLibrary,
    PinAuth,
    SetPin,
    ShareQr {
        title: String,
        content: String,
        caption: String,
    },
    LanSync {
        url: String,
        pin: String,
    },
}

struct App {
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    text: TextSystem,
    renderer: Rgba8Renderer,
    text_cache: ui::TextCache,
    input_view: ui::InputView,
    input_rect: Option<LogicalRect>,
    field_focused: bool,
    caret_on: bool,
    next_caret: Instant,
    next_visual: Instant,
    next_quality_save: Instant,
    hover: Option<Action>,
    pressed: Option<Action>,
    settings: Settings,
    page: Page,
    modal: Option<Modal>,
    edit: TextEdit,
    edit_scroll: f64,
    hits: Vec<(LogicalRect, Action)>,
    disabled_hits: Vec<(LogicalRect, String)>,
    disabled_hint: Option<String>,
    pointer: LogicalPoint,
    modifiers: ModifiersState,
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Update>,
    worker: Option<std::thread::JoinHandle<()>>,
    updates: UiUpdates,
    next_subscription_refresh: std::collections::HashMap<u64, Instant>,
    refreshing_subscription: Option<u64>,
    refresh_ticket: u64,
    pending_subscription: Option<(u64, String, Result<ImportReport, String>)>,
    subscription_errors: std::collections::HashSet<u64>,
    subscription_changes_pending: bool,
    antigravity_checking: bool,
    antigravity_result: Option<(String, String)>,
    connected: bool,
    system_proxy_active: bool,
    busy: bool,
    metrics: Metrics,
    history: VecDeque<(f64, f64)>,
    notice: String,
    logs: VecDeque<String>,
    offset: usize,
    started: Option<Instant>,
    tunnel_reachable: Option<bool>,
    compact: bool,
    autostart: bool,
    start_hidden: bool,
    restart_pending: bool,
    connect_after_restart: bool,
    smoke_tray: bool,
    rule_target: usize,
    editing_rule: Option<usize>,
    editing_subscription: Option<u64>,
    note_draft: String,
    note_value: String,
    note_parent: Option<Modal>,
    pending_notes: Option<std::collections::BTreeMap<String, String>>,
    pending_groups: Option<Vec<Vec<String>>>,
    other_rule_values: String,
    antigravity_template: bool,
    imported_routing: Option<multilink_core::RoutingConfig>,
    selecting_target: bool,
    lan_sync_session: Option<lan_sync::LanSyncSession>,
    lan_sync_rx: Option<mpsc::Receiver<String>>,
    target_page: usize,
    scroll_bar: Option<scrolling::ScrollBar>,
    scroll_drag: Option<f64>,
    #[cfg(windows)]
    tray_labels: Vec<(tray_icon::menu::MenuItem, &'static str)>,
    logo: Arc<[u8]>,
    #[cfg(windows)]
    tray: Option<tray_icon::TrayIcon>,
    #[cfg(windows)]
    tray_visual: std::cell::Cell<u8>,
    #[cfg(windows)]
    tray_connect: Option<tray_icon::menu::MenuItem>,
    #[cfg(windows)]
    tray_rule: Option<tray_icon::menu::MenuItem>,
    pin_unlocked: bool,
    pin_target_page: Option<Page>,
    initial_autoconnect_checked: bool,
    update_available: Option<String>,
    update_url: Option<String>,
    checking_update: bool,
}
const SUBSCRIPTION_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const SUBSCRIPTION_RETRY: Duration = Duration::from_secs(15 * 60);

impl App {
    fn managed_subscription(&self, id: u64) -> bool {
        self.settings
            .access
            .as_ref()
            .is_some_and(|a| a.subscription_id == id)
    }

    fn refresh_subscription(&mut self, id: u64) {
        if self.refreshing_subscription.is_some() || self.managed_subscription(id) {
            return;
        }
        let Some(url) = self
            .settings
            .subscriptions
            .iter()
            .find(|source| source.id == id)
            .map(|source| source.url.clone())
        else {
            return;
        };
        self.refreshing_subscription = Some(id);
        self.refresh_ticket += 1;
        let ticket = self.refresh_ticket;
        let updates = self.updates.clone();
        let connected = self.connected;
        std::thread::spawn(move || {
            let result = multilink_mihomo::fetch_subscription_with_proxy(&url, connected)
                .and_then(|body| multilink_core::import_subscription(&body));
            let _ = updates.send(Update::SubscriptionFetched(ticket, id, url, result));
        });
        self.redraw();
    }

    fn poll_lan_sync(&mut self) {
        if let Some(rx) = &self.lan_sync_rx {
            if let Ok(payload) = rx.try_recv() {
                if let Some(session) = self.lan_sync_session.take() {
                    session.stop();
                }
                self.lan_sync_rx = None;
                self.modal = None;
                let _ = self.tx.send(Command::Import(payload, None));
                self.notify("Получена конфигурация по сети. Выполняется импорт...");
                self.redraw();
            }
        }
    }

    fn poll_subscriptions(&mut self) {
        if self.busy || self.modal.is_some() || self.smoke_tray || self.restart_pending {
            return;
        }
        if let Some((id, url, result)) = self.pending_subscription.take() {
            self.refreshing_subscription = None;
            if self
                .settings
                .subscriptions
                .iter()
                .any(|s| s.id == id && s.url == url)
                && !self.managed_subscription(id)
            {
                let result = result.and_then(|report| {
                    let old: Vec<_> = self
                        .settings
                        .nodes
                        .iter()
                        .filter(|node| node.subscription_ids.contains(&id))
                        .map(|node| (node.name.clone(), node.proxy.clone()))
                        .collect();
                    multilink_core::update_subscription(
                        &mut self.settings,
                        Some(id),
                        &url,
                        report.nodes,
                    )?;
                    let changed = !old.iter().map(|(name, proxy)| (name, proxy)).eq(self
                        .settings
                        .nodes
                        .iter()
                        .filter(|node| node.subscription_ids.contains(&id))
                        .map(|node| (&node.name, &node.proxy)));
                    for warning in report.skipped {
                        self.logs.push_front(warning);
                    }
                    self.save();
                    Ok(changed)
                });
                let interval = match result {
                    Ok(changed) => {
                        self.subscription_errors.remove(&id);
                        self.subscription_changes_pending |= self.connected && changed;
                        if changed {
                            self.antigravity_result = None;
                        }
                        self.notify(if self.connected && changed {
                            "Список серверов обновлён · применится при следующем подключении"
                        } else {
                            "Список серверов подписки обновлён"
                        });
                        SUBSCRIPTION_INTERVAL
                    }
                    Err(error) => {
                        self.subscription_errors.insert(id);
                        self.notify(error);
                        SUBSCRIPTION_RETRY
                    }
                };
                self.next_subscription_refresh
                    .insert(id, Instant::now() + interval);
            }
            self.redraw();
        }
        if self.window.is_some() && self.refreshing_subscription.is_none() {
            let due = self
                .settings
                .subscriptions
                .iter()
                .find(|s| {
                    !self.managed_subscription(s.id)
                        && self
                            .next_subscription_refresh
                            .get(&s.id)
                            .is_none_or(|time| Instant::now() >= *time)
                })
                .map(|s| s.id);
            if let Some(id) = due {
                self.refresh_subscription(id);
            }
        }
    }

    fn new(wake: Option<EventLoopProxy<()>>) -> Self {
        let (tx, commands) = mpsc::channel();
        let (updates, rx) = mpsc::channel();
        let updates = UiUpdates {
            channel: updates,
            wake,
        };
        let worker_updates = updates.clone();
        let worker = std::thread::spawn(move || worker(commands, worker_updates));
        let loaded = multilink_platform::load()
            .map_err(|_| "Не удалось открыть защищённые настройки".to_owned())
            .and_then(|data| match data {
                Some(data) => serde_json::from_slice::<Settings>(&data)
                    .map_err(|_| "Файл настроек повреждён; он сохранён на диске".to_owned()),
                None => Ok(Settings::default()),
            });
        let (settings, notice) = match loaded {
            Ok(mut s) => match multilink_core::normalize_direct_rules(&s.rules) {
                Ok(rules) => {
                    s.rules = rules;
                    multilink_core::initialize_ru_routes(&mut s);
                    multilink_core::retain_rule_groups(&mut s.rule_groups, &s.rules);
                    let notice = if s.nodes.is_empty() {
                        "Введите ключ или добавьте свою подписку"
                    } else {
                        "Готово к подключению"
                    };
                    (s, notice.into())
                }
                Err(e) => (s, e),
            },
            Err(e) => (Settings::default(), e),
        };
        let mut renderer = Rgba8Renderer::new();
        icons::register(&mut renderer);
        let first_run = settings.nodes.is_empty();
        let app = Self {
            window: None,
            updates,
            next_subscription_refresh: Default::default(),
            refreshing_subscription: None,
            refresh_ticket: 0,
            pending_subscription: None,
            subscription_errors: Default::default(),
            subscription_changes_pending: false,
            antigravity_checking: false,
            antigravity_result: None,
            surface: None,
            text: TextSystem::new(),
            renderer,
            text_cache: ui::TextCache::default(),
            input_view: ui::InputView::default(),
            input_rect: None,
            field_focused: true,
            caret_on: true,
            next_caret: Instant::now() + Duration::from_millis(500),
            next_visual: Instant::now(),
            next_quality_save: Instant::now() + Duration::from_secs(60),
            hover: None,
            pressed: None,
            settings,
            page: if first_run {
                Page::Access
            } else {
                Page::Dashboard
            },
            modal: None,
            edit: TextEdit::new(4 * 1024 * 1024),
            edit_scroll: 0.0,
            hits: Vec::new(),
            disabled_hits: Vec::new(),
            disabled_hint: None,
            pointer: LogicalPoint::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            tx,
            rx,
            worker: Some(worker),
            connected: false,
            system_proxy_active: false,
            busy: false,
            metrics: Metrics::default(),
            history: VecDeque::new(),
            notice,
            logs: VecDeque::new(),
            offset: 0,
            started: None,
            tunnel_reachable: None,
            compact: !first_run,
            autostart: autostart_enabled().unwrap_or(false),
            start_hidden: std::env::args()
                .any(|arg| arg == "--start-hidden" || arg == "--smoke-tray"),
            restart_pending: false,
            connect_after_restart: false,
            smoke_tray: std::env::args().any(|arg| arg == "--smoke-tray"),
            rule_target: 0,
            editing_rule: None,
            editing_subscription: None,
            note_draft: String::new(),
            note_value: String::new(),
            note_parent: None,
            pending_notes: None,
            pending_groups: None,
            other_rule_values: String::new(),
            antigravity_template: false,
            imported_routing: None,
            selecting_target: false,
            lan_sync_session: None,
            lan_sync_rx: None,
            target_page: 0,
            scroll_bar: None,
            scroll_drag: None,
            #[cfg(windows)]
            tray_labels: Vec::new(),
            logo: image::load_from_memory(include_bytes!("../../../logo.png"))
                .expect("embedded logo")
                .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
                .to_rgba8()
                .into_raw()
                .into(),
            #[cfg(windows)]
            tray: None,
            #[cfg(windows)]
            tray_visual: std::cell::Cell::new(255),
            #[cfg(windows)]
            tray_connect: None,
            #[cfg(windows)]
            tray_rule: None,
            pin_unlocked: false,
            pin_target_page: None,
            initial_autoconnect_checked: false,
            update_available: None,
            update_url: None,
            checking_update: false,
        };
        let check_updates = app.updates.clone();
        std::thread::spawn(move || {
            let res = multilink_mihomo::check_github_release(env!("CARGO_PKG_VERSION"), false);
            let _ = check_updates.send(Update::UpdateCheckResult(res));
        });
        app
    }
    fn pin_locked(&self) -> bool {
        self.settings.pin_code.is_some() && !self.pin_unlocked
    }
    fn save(&mut self) {
        let _ = self.tx.send(Command::Save(self.settings.clone()));
    }
    fn request_elevated_restart(&mut self, connect_after_restart: bool) -> bool {
        let (reply, result) = mpsc::channel();
        if self
            .tx
            .send(Command::SaveForRestart(self.settings.clone(), reply))
            .is_err()
        {
            self.notify("Не удалось сохранить настройки перед перезапуском");
            return false;
        }
        match result.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => {}
            _ => {
                self.notify("Не удалось сохранить настройки перед перезапуском");
                return false;
            }
        }
        match multilink_platform::restart_elevated(connect_after_restart, self.compact) {
            Ok(()) => {
                self.restart_pending = true;
                true
            }
            Err(_) => {
                self.notify("Запрос прав администратора отменён или не выполнен");
                false
            }
        }
    }
    fn notify(&mut self, message: impl Into<String>) {
        self.notice = message.into();
        self.logs.push_front(self.notice.clone());
        self.logs.truncate(100);
    }
    fn drain(&mut self) {
        while let Ok(update) = self.rx.try_recv() {
            if let Update::AccessReady(token, doc) = update {
                self.busy = false;
                match multilink_core::install_access(&mut self.settings, token, doc) {
                    Ok(()) => {
                        self.modal = None;
                        self.edit.clear();
                        self.page = Page::Access;
                        self.save();
                        self.notify("Доступ активен. Можно подключаться");
                    }
                    Err(error) => self.notify(error),
                }
                continue;
            }
            match update {
                Update::SubscriptionFetched(ticket, id, url, result) => {
                    if self.refresh_ticket == ticket && self.refreshing_subscription == Some(id) {
                        self.pending_subscription = Some((id, url, result));
                    }
                }
                Update::AntigravityChecked(name, result) => {
                    self.antigravity_checking = false;
                    let message = match result {
                        Ok(multilink_mihomo::AntigravityCheck::Blocked(status)) => {
                            format!("Регион заблокирован (HTTP {status}) · {name}")
                        }
                        Ok(multilink_mihomo::AntigravityCheck::Responded(status)) => {
                            format!("Блокировка региона не обнаружена (HTTP {status}) · {name}")
                        }
                        Ok(multilink_mihomo::AntigravityCheck::Inconclusive(status)) => {
                            format!("Проверка региона не дала ответа (HTTP {status}) · {name}")
                        }
                        Err(error) => format!("{error} · {name}"),
                    };
                    self.antigravity_result =
                        Some((multilink_core::server_route_target(&name), message.clone()));
                    self.logs.push_front(message);
                    self.logs.truncate(100);
                }
                Update::AccessSynced(token, doc) => {
                    match multilink_core::install_access(&mut self.settings, token, doc) {
                        Ok(()) => self.save(),
                        Err(error) => self.notify(error),
                    }
                }
                Update::AccessChecked(expiry) => {
                    if let Some(a) = &mut self.settings.access {
                        a.expires_at = expiry;
                    }
                    self.save();
                }
                Update::AccessReady(_, _) => unreachable!(),
                Update::AccessRejected => {
                    self.busy = false;
                    if let Some(a) = &mut self.settings.access {
                        a.blocked = true;
                    }
                    self.save();
                    self.notify("Доступ истёк или отозван. Обновите подписку у администратора");
                }
                Update::Connected(system_proxy) => {
                    self.subscription_changes_pending = false;
                    self.connected = true;
                    #[cfg(windows)]
                    if let Some(item) = &self.tray_connect {
                        item.set_text("Отключиться");
                    }
                    #[cfg(windows)]
                    if let Some(tray) = &self.tray {
                        let _ = tray.set_tooltip(Some("Everty MultiLink · подключено"));
                    }
                    self.system_proxy_active = system_proxy;
                    self.busy = false;
                    self.started = Some(Instant::now());
                    self.notify(if self.settings.tun && system_proxy {
                        "Подключено · TUN и системный прокси Windows включены"
                    } else if self.settings.tun {
                        "Подключено · TUN-адаптер активен"
                    } else if system_proxy {
                        "Подключено · системный прокси Windows включён"
                    } else {
                        "Подключено · локальный HTTP / SOCKS5 127.0.0.1:17890"
                    });
                }
                Update::Disconnected => {
                    self.subscription_changes_pending = false;
                    self.save();
                    let automatic = self.system_proxy_active;
                    self.connected = false;
                    #[cfg(windows)]
                    if let Some(item) = &self.tray_connect {
                        item.set_text("Подключиться");
                    }
                    #[cfg(windows)]
                    if let Some(tray) = &self.tray {
                        let _ = tray.set_tooltip(Some("Everty MultiLink · отключено"));
                    }
                    self.system_proxy_active = false;
                    self.busy = false;
                    self.started = None;
                    self.metrics = Metrics::default();
                    for n in &mut self.settings.nodes {
                        n.delay = None;
                    }
                    self.notify(if self.settings.tun && automatic {
                        "Отключено · TUN остановлен, настройки прокси восстановлены"
                    } else if self.settings.tun {
                        "Отключено · TUN остановлен"
                    } else if automatic {
                        "Отключено · исходные настройки прокси восстановлены"
                    } else {
                        "Локальный прокси остановлен"
                    });
                }
                Update::Error(error) => {
                    self.busy = false;
                    self.connected = false;
                    #[cfg(windows)]
                    if let Some(item) = &self.tray_connect {
                        item.set_text("Подключиться");
                    }
                    #[cfg(windows)]
                    if let Some(tray) = &self.tray {
                        let _ = tray.set_tooltip(Some("Everty MultiLink · отключено"));
                    }
                    self.system_proxy_active = false;
                    self.started = None;
                    self.notify(error);
                }
                Update::SaveError => self.notify("Не удалось сохранить настройки"),
                Update::Notice(message) => self.notify(message),
                Update::OperationError(message) => {
                    self.busy = false;
                    self.notify(message);
                }
                Update::RoutingApplied(rules, policy, change) => {
                    self.settings.rules = rules;
                    self.settings.policy = policy;
                    self.busy = false;
                    self.modal = None;
                    self.editing_rule = None;
                    self.input_rect = None;
                    self.input_view = ui::InputView::default();
                    self.edit.clear();
                    self.commit_route_notes();
                    self.save();
                    self.notify(if change.retry_tabs {"Исключения применены · обновите старые вкладки, чтобы открыть новые соединения".into()}else{format!("Маршрутизация применена · переподключено соединений: {}",change.closed_connections)});
                    let missing = multilink_core::unavailable_route_targets(&self.settings);
                    if !missing.is_empty() {
                        self.notify(format!(
                            "Недоступных привязок: {} · временно используется общий VPN-пул",
                            missing.len()
                        ));
                    }
                }
                Update::RoutingFailed(error) => {
                    self.busy = false;
                    self.notify(error);
                }
                Update::Imported(report, url, source) => {
                    let count = report.nodes.len();
                    if let Some(url) = url {
                        match multilink_core::update_subscription(
                            &mut self.settings,
                            source,
                            &url,
                            report.nodes,
                        ) {
                            Ok(id) => {
                                if self.refreshing_subscription == Some(id) {
                                    self.refreshing_subscription = None;
                                    self.pending_subscription = None;
                                }
                                self.next_subscription_refresh
                                    .insert(id, Instant::now() + SUBSCRIPTION_INTERVAL);
                                self.subscription_errors.remove(&id);
                            }
                            Err(error) => {
                                self.busy = false;
                                self.notify(error);
                                continue;
                            }
                        }
                    } else {
                        multilink_core::merge_nodes(&mut self.settings.nodes, report.nodes);
                    }
                    self.settings.imports += 1;
                    self.busy = false;
                    self.modal = None;
                    self.input_rect = None;
                    self.input_view = ui::InputView::default();
                    self.edit.clear();
                    self.page = if self.settings.subscriptions.is_empty() {
                        Page::Servers
                    } else {
                        Page::Subscriptions
                    };
                    self.save();
                    for warning in &report.skipped {
                        self.logs.push_front(warning.clone());
                    }
                    self.notify(format!(
                        "Проверено серверов: {count} · несовместимых: {} · подробности в журнале",
                        report.skipped.len()
                    ));
                }
                Update::Metrics(metrics, delays) => {
                    self.history.push_back((metrics.down_rate, metrics.up_rate));
                    if self.history.len() > 90 {
                        self.history.pop_front();
                    }
                    self.metrics = metrics;
                    for (name, delay, quality) in delays {
                        if let Some(n) = self.settings.nodes.iter_mut().find(|n| n.name == name) {
                            n.delay = delay;
                            n.quality = quality;
                        }
                    }
                    if Instant::now() >= self.next_quality_save {
                        self.save();
                        self.next_quality_save = Instant::now() + Duration::from_secs(60);
                    }
                }
                Update::TunnelReachable(reachable) => {
                    let changed = self.tunnel_reachable != Some(reachable);
                    self.tunnel_reachable = Some(reachable);
                    if changed {
                        self.notify(if reachable {
                            "Связь через VPN восстановлена"
                        } else {
                            "Нет ответа через VPN · проверьте маршрут или сервер"
                        });
                    }
                    self.redraw();
                }
                Update::UpdateCheckResult(res) => {
                    let manual = self.checking_update;
                    self.checking_update = false;
                    match res {
                        Ok(Some((tag, url))) => {
                            self.update_available = Some(tag.clone());
                            self.update_url = Some(url);
                            self.notify(format!("Доступно обновление: {tag}"));
                        }
                        Ok(None) => {
                            self.update_available = None;
                            if manual {
                                self.notify("У вас установлена последняя версия");
                            }
                        }
                        Err(e) => {
                            if manual {
                                self.notify(format!("Не удалось проверить обновления: {e}"));
                            }
                        }
                    }
                    self.redraw();
                }
            }
            self.sync_tray();
        }
    }
    fn action(&mut self, action: Action) {
        self.redraw();
        if self.pin_locked() {
            match action {
                Action::Nav(page)
                    if matches!(
                        page,
                        Page::Settings | Page::Servers | Page::Subscriptions | Page::Routing
                    ) =>
                {
                    self.pin_target_page = Some(page);
                    self.modal = Some(Modal::PinAuth);
                    self.edit.clear();
                    self.edit_scroll = 0.0;
                    self.field_focused = true;
                    self.reset_caret();
                    return;
                }
                Action::ClearServers
                | Action::DeleteRule(_)
                | Action::EditRule(_)
                | Action::AddRule
                | Action::AddRuDomains
                | Action::AddProcessRule
                | Action::AddAntigravity
                | Action::ImportRoutes
                | Action::ReplaceRoutes
                | Action::ExportSettings
                | Action::ImportSettings
                | Action::ToggleSystemProxy
                | Action::ToggleTun
                | Action::ToggleAutostart
                | Action::ToggleAutoconnect
                | Action::ToggleRulesOnly
                | Action::Policy
                | Action::OpenSetPin
                | Action::RemovePin
                | Action::OpenLanSync
                | Action::ShareNode(_)
                | Action::ShareSubscription(_)
                | Action::DeleteSubscription(_)
                | Action::RenameSubscription(_)
                | Action::EditSubscriptionUrl(_)
                | Action::RefreshSubscription(_)
                | Action::EnableAllServers
                | Action::DisableAllServers
                | Action::RankServers
                | Action::Import => {
                    self.modal = Some(Modal::PinAuth);
                    self.edit.clear();
                    self.edit_scroll = 0.0;
                    self.field_focused = true;
                    self.reset_caret();
                    self.notify("Действие защищено PIN-кодом");
                    return;
                }
                _ => {}
            }
        }
        match action {
            Action::HelpSection(section) => {
                self.offset = section.min(help::COUNT - 1);
                self.redraw();
            }
            Action::Language => {
                self.modal = Some(Modal::Language);
                self.input_rect = None;
            }
            Action::SelectLanguage(index) => {
                if let Some((key, _)) = i18n::OPTIONS.get(index) {
                    self.settings.language = (*key).into();
                }
                self.modal = None;
                self.save();
                self.sync_tray();
                self.redraw();
            }
            Action::ActivateAccess if !self.busy && !self.connected => {
                self.modal = Some(Modal::AccessCode);
                self.edit.set_text(
                    self.settings
                        .pending_activation
                        .as_ref()
                        .map(|p| p.code.as_str())
                        .unwrap_or(""),
                );
                self.field_focused = true;
                self.selecting_target = false;
                self.edit_scroll = 0.0;
                self.reset_caret();
            }
            Action::RefreshAccess if !self.busy && !self.connected => {
                if let Some(access) = &self.settings.access {
                    self.busy = true;
                    let _ = self.tx.send(Command::RefreshAccess(access.clone()));
                    self.notify("Обновление доступа…");
                }
            }
            Action::Nav(page) => {
                self.scroll_bar = None;
                self.scroll_drag = None;
                self.page = page;
                self.offset = 0;
            }
            Action::OpenAboutSite | Action::OpenGitHub | Action::OpenDonate => {
                let url = match action {
                    Action::OpenGitHub => "https://github.com/vaalimusic/EvertyMultiLink",
                    Action::OpenDonate => "https://boosty.to/vaali/donate",
                    _ => "https://desk.everty.ru/opensource",
                };
                if multilink_platform::open_project_site(url).is_err() {
                    self.notify(format!("Не удалось открыть браузер: {url}"));
                }
            }
            Action::CopyQrContent => {
                let text = match &self.modal {
                    Some(Modal::ShareQr { content, .. }) => Some(content.clone()),
                    Some(Modal::LanSync { url, .. }) => Some(url.clone()),
                    _ => None,
                };
                if let Some(text) = text {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(text);
                        self.notify("Скопировано в буфер обмена");
                    }
                }
            }
            Action::ShareNode(index) => {
                if let Some(node) = self.settings.nodes.get(index) {
                    if let Some(uri) = node.to_vless_uri() {
                        self.modal = Some(Modal::ShareQr {
                            title: format!("Поделиться: {}", node.name),
                            content: uri,
                            caption: "Отсканируйте камерой смартфона или скопируйте VLESS-ссылку:".into(),
                        });
                        self.redraw();
                    } else {
                        self.notify("Не удалось преобразовать узел в ссылку");
                    }
                }
            }
            Action::ShareSubscription(id) => {
                if let Some(sub) = self.settings.subscriptions.iter().find(|s| s.id == id) {
                    self.modal = Some(Modal::ShareQr {
                        title: format!("Подписка: {}", sub.name),
                        content: sub.url.clone(),
                        caption: "Отсканируйте камерой смартфона или скопируйте ссылку на подписку:".into(),
                    });
                    self.redraw();
                }
            }
            Action::OpenLanSync => {
                let (tx, rx) = std::sync::mpsc::channel();
                match lan_sync::start_lan_sync(tx) {
                    Ok(session) => {
                        let url = session.url.clone();
                        let pin = session.pin.clone();
                        self.lan_sync_session = Some(session);
                        self.lan_sync_rx = Some(rx);
                        self.modal = Some(Modal::LanSync { url, pin });
                        self.redraw();
                    }
                    Err(e) => {
                        self.notify(format!("Ошибка запуска: {e}"));
                    }
                }
            }
            Action::RefreshSubscription(id) if !self.busy => {
                if self
                    .settings
                    .access
                    .as_ref()
                    .is_some_and(|a| a.subscription_id == id)
                {
                    if !self.connected {
                        self.action(Action::RefreshAccess);
                    } else {
                        self.notify("Управляемый доступ обновляется отдельно");
                    }
                    return;
                }
                if let Some(active) = self.refreshing_subscription {
                    if id != active {
                        self.next_subscription_refresh.insert(id, Instant::now());
                    }
                } else {
                    self.refresh_subscription(id);
                }
            }
            Action::EditSubscriptionUrl(id)
            | Action::RenameSubscription(id)
            | Action::DeleteSubscription(id)
                if !self.busy && !self.connected =>
            {
                if matches!(action, Action::EditSubscriptionUrl(_))
                    && self
                        .settings
                        .access
                        .as_ref()
                        .is_some_and(|a| a.subscription_id == id)
                {
                    self.page = Page::Access;
                    return;
                }
                if let Some(source) = self.settings.subscriptions.iter().find(|s| s.id == id) {
                    self.editing_subscription = Some(id);
                    self.modal = Some(match action {
                        Action::RenameSubscription(_) => Modal::SubscriptionName,
                        Action::DeleteSubscription(_) => Modal::SubscriptionDelete,
                        _ => Modal::SubscriptionUrl,
                    });
                    self.edit
                        .set_text(if self.modal == Some(Modal::SubscriptionUrl) {
                            &source.url
                        } else {
                            &source.name
                        });
                    if self.modal == Some(Modal::SubscriptionUrl) {
                        self.edit.set_caret(0, false);
                    }
                    self.edit_scroll = 0.0;
                    self.field_focused = true;
                    self.reset_caret();
                }
            }
            Action::RefreshSubscription(_)
            | Action::EditSubscriptionUrl(_)
            | Action::RenameSubscription(_)
            | Action::DeleteSubscription(_) => {
                self.notify("Отключитесь перед изменением подписки");
            }
            Action::Compact => self.set_compact(true),
            Action::Expanded => self.set_compact(false),
            Action::ToggleAutostart => match set_autostart(!self.autostart) {
                Ok(()) => {
                    self.autostart = !self.autostart;
                    self.notify(if self.autostart {
                        "Автозапуск включён"
                    } else {
                        "Автозапуск выключен"
                    });
                }
                Err(_) => self.notify("Не удалось изменить автозапуск"),
            },
            Action::RuleTarget => self.selecting_target = !self.selecting_target,
            Action::ExportRoutes if !self.busy => {
                match multilink_platform::routing_file_dialog(true) {
                    Ok(Some(path)) => {
                        let result = multilink_core::RoutingConfig::from_settings(&self.settings)
                            .encode()
                            .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
                        self.notify(match result {
                            Ok(()) => "Маршруты и заметки экспортированы".into(),
                            Err(e) => format!("Ошибка экспорта: {e}"),
                        });
                    }
                    Ok(None) => {}
                    Err(e) => self.notify(format!("Ошибка диалога файла: {e}")),
                }
            }
            Action::ImportRoutes if !self.busy => {
                match multilink_platform::routing_file_dialog(false) {
                    Ok(Some(path)) => {
                        use std::io::Read;
                        let result = std::fs::File::open(path)
                            .map_err(|e| e.to_string())
                            .and_then(|file| {
                                let mut text = String::new();
                                file.take(2 * 1024 * 1024 + 1)
                                    .read_to_string(&mut text)
                                    .map_err(|e| e.to_string())?;
                                multilink_core::RoutingConfig::decode(&text)
                            });
                        match result {
                            Ok(config) => {
                                self.imported_routing = Some(config);
                                self.modal = Some(Modal::RouteImport);
                                self.edit.set_text("Импорт маршрутов");
                                self.editing_rule = None;
                                self.pending_notes = None;
                                self.selecting_target = false;
                            }
                            Err(e) => self.notify(format!("Импорт отменён: {e}")),
                        }
                    }
                    Ok(None) => {}
                    Err(e) => self.notify(format!("Ошибка диалога файла: {e}")),
                }
            }
            Action::ExportSettings if !self.busy => {
                match multilink_platform::settings_file_dialog(true) {
                    Ok(Some(path)) => {
                        let backup = serde_json::json!({
                            "app": "EvertyMultiLink",
                            "version": env!("CARGO_PKG_VERSION"),
                            "exported_at": multilink_core::unix_now(),
                            "settings": self.settings,
                        });
                        let result = serde_json::to_string_pretty(&backup)
                            .map_err(|e| e.to_string())
                            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
                        self.notify(match result {
                            Ok(()) => "Все настройки успешно экспортированы".into(),
                            Err(e) => format!("Ошибка экспорта: {e}"),
                        });
                    }
                    Ok(None) => {}
                    Err(e) => self.notify(format!("Ошибка диалога файла: {e}")),
                }
            }
            Action::ImportSettings if !self.busy => {
                match multilink_platform::settings_file_dialog(false) {
                    Ok(Some(path)) => {
                        use std::io::Read;
                        let result = std::fs::File::open(path)
                            .map_err(|e| e.to_string())
                            .and_then(|file| {
                                let mut text = String::new();
                                file.take(16 * 1024 * 1024 + 1)
                                    .read_to_string(&mut text)
                                    .map_err(|e| e.to_string())?;
                                let val: serde_json::Value = serde_json::from_str(&text)
                                    .map_err(|e| format!("Некорректный JSON: {e}"))?;
                                let settings_val = if let Some(inner) = val.get("settings") {
                                    inner.clone()
                                } else {
                                    val
                                };
                                serde_json::from_value::<Settings>(settings_val)
                                    .map_err(|e| format!("Неверный формат настроек: {e}"))
                            });
                        match result {
                            Ok(mut loaded) => {
                                if let Ok(rules) = multilink_core::normalize_direct_rules(&loaded.rules) {
                                    loaded.rules = rules;
                                }
                                self.settings = loaded;
                                self.save();
                                if self.connected {
                                    let _ = self.tx.send(Command::ApplySettings(self.settings.clone()));
                                }
                                self.notify("Все настройки успешно импортированы");
                            }
                            Err(e) => self.notify(format!("Ошибка импорта: {e}")),
                        }
                    }
                    Ok(None) => {}
                    Err(e) => self.notify(format!("Ошибка диалога файла: {e}")),
                }
            }
            Action::ReplaceRoutes if !self.busy && self.modal == Some(Modal::RouteImport) => {
                self.apply_imported_routes(true)
            }
            Action::TargetPage(page) => {
                let count = self.rule_targets().len().div_ceil(18).max(1);
                self.target_page = page.min(count - 1);
            }
            Action::SelectRuleTarget(index) => {
                if index < self.rule_targets().len() {
                    self.rule_target = index;
                }
                self.selecting_target = false;
                self.target_page = self.rule_target / 18;
            }
            Action::Close if !self.busy && self.modal == Some(Modal::Note) => {
                self.modal = self.note_parent.take();
                self.edit.set_text(&self.note_value);
                self.edit_scroll = 0.0;
            }
            Action::Close if !self.busy => {
                if let Some(session) = self.lan_sync_session.take() {
                    session.stop();
                }
                self.lan_sync_rx = None;
                self.modal = None;
                self.editing_rule = None;
                self.editing_subscription = None;
                self.input_rect = None;
                self.input_view = ui::InputView::default();
                self.edit.clear();
                self.rule_target = 0;
                self.selecting_target = false;
                self.target_page = 0;
                self.note_draft.clear();
                self.pending_notes = None;
                self.pending_groups = None;
                self.imported_routing = None;
            }
            Action::Close => {}
            Action::Import
            | Action::AddRule
            | Action::AddRuDomains
            | Action::AddProcessRule
            | Action::AddAntigravity
                if !self.busy && (!matches!(action, Action::Import) || !self.connected) =>
            {
                self.editing_rule = None;
                self.note_draft.clear();
                self.pending_notes = None;
                self.pending_groups = None;
                self.other_rule_values.clear();
                self.antigravity_template = false;
                self.modal = Some(match action {
                    Action::Import => Modal::Import,
                    Action::AddRuDomains => Modal::RuRule,
                    Action::AddProcessRule | Action::AddAntigravity => Modal::ProcessRule,
                    _ => Modal::Rule,
                });
                self.edit.clear();
                if self.modal == Some(Modal::RuRule) {
                    self.edit.set_text("Российские сайты и домены");
                }
                self.edit_scroll = 0.0;
                self.rule_target = 0;
                if action == Action::AddAntigravity {
                    self.edit.set_text("Antigravity.exe");
                    if let Some(index) = self.settings.rules.iter().position(|rule| {
                        multilink_core::rule_key(rule) == "PROCESS-NAME,antigravity.exe"
                    }) {
                        self.action(Action::EditRule(index));
                        if !self.antigravity_template {
                            self.other_rule_values = "googleapis.com".into();
                        }
                    } else {
                        let preferred = self
                            .settings
                            .nodes
                            .iter()
                            .find(|node| {
                                node.enabled
                                    && multilink_core::node_country(&node.name).as_deref()
                                        == Some("US")
                            })
                            .map(|node| multilink_core::server_route_target(&node.name));
                        self.rule_target = preferred
                            .and_then(|target| {
                                self.rule_targets().iter().position(|(id, _)| *id == target)
                            })
                            .unwrap_or(1);
                        self.other_rule_values = "googleapis.com".into();
                    }
                    self.antigravity_template = true;
                }
                self.selecting_target = false;
                self.target_page = self.rule_target / 18;
                self.field_focused = true;
                self.reset_caret();
            }
            Action::RuleField(process)
                if !self.busy && matches!(self.modal, Some(Modal::Rule | Modal::ProcessRule)) =>
            {
                if process != (self.modal == Some(Modal::ProcessRule)) {
                    let previous = self.edit.text().to_owned();
                    self.edit.set_text(&self.other_rule_values);
                    self.other_rule_values = previous;
                    self.modal = Some(if process {
                        Modal::ProcessRule
                    } else {
                        Modal::Rule
                    });
                    self.edit_scroll = 0.0;
                    self.field_focused = true;
                    self.reset_caret();
                }
            }
            Action::CheckAntigravity if !self.busy && !self.antigravity_checking => {
                let target = self
                    .rule_targets()
                    .get(self.rule_target)
                    .map(|(id, _)| id.clone());
                let mut effective = self.settings.clone();
                multilink_core::exclude_expired_access(&mut effective);
                let node = effective.nodes.into_iter().find(|n| {
                    n.enabled
                        && target.as_deref() == Some(&multilink_core::server_route_target(&n.name))
                });
                if let Some(node) = node {
                    let updates = self.updates.clone();
                    self.antigravity_checking = true;
                    std::thread::spawn(move || {
                        let result = multilink_mihomo::check_antigravity(&node);
                        let _ = updates.send(Update::AntigravityChecked(node.name, result));
                    });
                } else {
                    self.notify("Для проверки выберите конкретный включённый сервер");
                }
            }
            Action::Import | Action::AddRule | Action::AddRuDomains | Action::AddProcessRule => {
                self.notify("Отключитесь перед изменением конфигурации")
            }
            Action::EditRule(index) if !self.busy => {
                if let Some(rule) = self.settings.rules.get(index).cloned() {
                    let parts: Vec<_> = rule.split(',').collect();
                    if let [kind, value, target, ..] = parts.as_slice() {
                        self.editing_rule = Some(index);
                        self.note_draft = self
                            .settings
                            .rule_notes
                            .get(&multilink_core::rule_key(&rule))
                            .cloned()
                            .unwrap_or_default();
                        self.pending_notes = None;
                        self.pending_groups = None;
                        self.other_rule_values.clear();
                        self.antigravity_template = false;
                        self.modal = Some(match *kind {
                            "RULE-SET" => Modal::RuRule,
                            "PROCESS-NAME" => Modal::ProcessRule,
                            _ => Modal::Rule,
                        });
                        self.edit.set_text(if *kind == "RULE-SET" {
                            "Российские сайты и домены"
                        } else {
                            value
                        });
                        if *kind != "RULE-SET" {
                            let keys = self.rule_members(index);
                            let mut processes = Vec::new();
                            let mut addresses = Vec::new();
                            for key in &keys {
                                if let Some((kind, value)) = key.split_once(',') {
                                    if kind == "PROCESS-NAME" {
                                        processes.push(value);
                                    } else {
                                        addresses.push(value);
                                    }
                                }
                            }
                            let process = *kind == "PROCESS-NAME";
                            let (current, other) = if process {
                                (processes.join(", "), addresses.join(", "))
                            } else {
                                (addresses.join(", "), processes.join(", "))
                            };
                            self.edit.set_text(&current);
                            self.other_rule_values = other;
                            self.antigravity_template =
                                self.settings.rule_groups.iter().any(|group| {
                                    group == &keys
                                        && group
                                            .iter()
                                            .any(|key| key == "PROCESS-NAME,antigravity.exe")
                                });
                        }
                        self.edit_scroll = 0.0;
                        self.selecting_target = false;
                        self.rule_target = self
                            .rule_targets()
                            .iter()
                            .position(|(id, _)| id == target)
                            .unwrap_or(0);
                        self.target_page = self.rule_target / 18;
                        self.field_focused = true;
                        self.reset_caret();
                    }
                }
            }
            Action::Paste => {
                self.field_focused = true;
                self.reset_caret();
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        self.edit.insert_multiline(&text);
                    }
                }
            }
            Action::RuleNote
                if !self.busy
                    && matches!(
                        self.modal,
                        Some(Modal::Rule | Modal::ProcessRule | Modal::RuRule)
                    ) =>
            {
                self.note_parent = self.modal.clone();
                self.note_value = self.edit.text().to_owned();
                self.edit.set_text(&self.note_draft);
                self.edit_scroll = 0.0;
                self.modal = Some(Modal::Note);
                self.field_focused = true;
                self.reset_caret();
            }
            Action::Submit if !self.busy => {
                if self.edit.is_empty()
                    && !matches!(
                        self.modal,
                        Some(Modal::Note | Modal::Rule | Modal::ProcessRule)
                    )
                {
                    return;
                }
                match self.modal {
                    Some(Modal::Note) => {
                        if self.edit.text().len() > 4096 {
                            self.notify("Заметка должна быть не длиннее 4096 байт");
                            return;
                        }
                        self.note_draft = self.edit.text().to_owned();
                        self.modal = self.note_parent.take();
                        self.edit.set_text(&self.note_value);
                        self.edit_scroll = 0.0;
                    }
                    Some(Modal::RouteImport) => self.apply_imported_routes(false),
                    Some(Modal::Import) => {
                        self.busy = true;
                        let _ = self
                            .tx
                            .send(Command::Import(self.edit.text().to_owned(), None));
                        self.notify("Загрузка и проверка подписки…");
                    }
                    Some(Modal::AccessCode) => {
                        match multilink_core::access_code(self.edit.text()) {
                            Ok(code) => {
                                let pending = self
                                    .settings
                                    .pending_activation
                                    .as_ref()
                                    .filter(|p| p.code == code)
                                    .cloned()
                                    .unwrap_or_else(|| multilink_core::PendingActivation {
                                        code: code.clone(),
                                        nonce: format!(
                                            "{}{}",
                                            uuid::Uuid::new_v4().simple(),
                                            uuid::Uuid::new_v4().simple()
                                        ),
                                    });
                                self.settings.pending_activation = Some(pending.clone());
                                self.edit.set_text(&code);
                                self.busy = true;
                                let _ = self
                                    .tx
                                    .send(Command::Activate(pending, self.settings.clone()));
                                self.notify("Активация ключа…");
                            }
                            Err(error) => self.notify(error),
                        }
                    }
                    Some(Modal::SubscriptionUrl) => {
                        match multilink_core::subscription_url(self.edit.text()) {
                            Ok(url) => {
                                self.busy = true;
                                let _ = self
                                    .tx
                                    .send(Command::Import(url, self.editing_subscription));
                                self.notify(
                                    "Проверяем новую ссылку; прежняя сохранится при ошибке…",
                                );
                            }
                            Err(error) => self.notify(error),
                        }
                    }
                    Some(Modal::SubscriptionName) => {
                        let name = self.edit.text().trim();
                        if name.chars().count() > 80 || name.chars().any(char::is_control) {
                            self.notify("Название: до 80 символов, без переносов строк");
                            return;
                        }
                        if let Some(source) = self
                            .settings
                            .subscriptions
                            .iter_mut()
                            .find(|s| Some(s.id) == self.editing_subscription)
                        {
                            source.name = name.into();
                        }
                        self.modal = None;
                        self.save();
                        self.notify("Название подписки сохранено");
                    }
                    Some(Modal::SubscriptionDelete) => {
                        if let Some(id) = self.editing_subscription.take() {
                            if self
                                .settings
                                .access
                                .as_ref()
                                .is_some_and(|a| a.subscription_id == id)
                            {
                                self.settings.access = None;
                            }
                            multilink_core::remove_subscription(&mut self.settings, id);
                            if self.refreshing_subscription == Some(id) {
                                self.refreshing_subscription = None;
                                self.pending_subscription = None;
                            }
                            self.next_subscription_refresh.remove(&id);
                            self.subscription_errors.remove(&id);
                            self.offset = 0;
                            self.modal = None;
                            self.save();
                            self.notify(
                                "Подписка удалена; общие и вручную добавленные серверы сохранены",
                            );
                        }
                    }
                    Some(Modal::PinAuth) => {
                        let entered = self.edit.text().trim();
                        if let Some(expected) = &self.settings.pin_code {
                            if entered == expected {
                                self.pin_unlocked = true;
                                self.modal = None;
                                self.edit.clear();
                                self.notify("PIN-код подтверждён · настройки разблокированы");
                                if let Some(target) = self.pin_target_page.take() {
                                    self.page = target;
                                    self.offset = 0;
                                }
                            } else {
                                self.edit.clear();
                                self.notify("Неверный PIN-код");
                            }
                        } else {
                            self.pin_unlocked = true;
                            self.modal = None;
                            self.edit.clear();
                        }
                    }
                    Some(Modal::SetPin) => {
                        let entered = self.edit.text().trim().to_string();
                        if entered.len() < 4 {
                            self.notify("PIN-код должен содержать не менее 4 символов");
                        } else {
                            self.settings.pin_code = Some(entered);
                            self.pin_unlocked = true;
                            self.modal = None;
                            self.edit.clear();
                            self.save();
                            self.notify("PIN-код успешно установлен");
                        }
                    }
                    Some(Modal::ClearLibrary) => {
                        self.settings.nodes.clear();
                        self.settings.subscriptions.clear();
                        self.refreshing_subscription = None;
                        self.pending_subscription = None;
                        self.next_subscription_refresh.clear();
                        self.subscription_errors.clear();
                        self.settings.access = None;
                        self.settings.pending_activation = None;
                        self.settings.imports = 0;
                        self.modal = None;
                        self.save();
                        self.notify("Библиотека серверов очищена");
                    }
                    Some(Modal::Language) => {}
                    Some(Modal::Rule) | Some(Modal::ProcessRule) => {
                        let target = self
                            .rule_targets()
                            .get(self.rule_target)
                            .map(|(id, _)| id.clone())
                            .unwrap_or_else(|| "DIRECT".into());
                        match self.parse_rule_fields(&target) {
                            Ok(rules) => self.submit_many_rules(rules),
                            Err(error) => self.notify(error),
                        }
                    }
                    Some(Modal::RuRule) => {
                        let targets = self.rule_targets();
                        let target = targets
                            .get(self.rule_target)
                            .map(|(id, _)| id.as_str())
                            .unwrap_or("DIRECT");
                        if self.editing_rule.is_some() {
                            self.submit_single_rule(format!("RULE-SET,RU-SITES,{target}"));
                        } else {
                            self.submit_many_rules(vec![
                                format!("RULE-SET,RU-SITES,{target}"),
                                format!("DOMAIN-SUFFIX,ru,{target}"),
                                format!("DOMAIN-SUFFIX,xn--p1ai,{target}"),
                            ]);
                        }
                    }
                    Some(Modal::ShareQr { .. }) | Some(Modal::LanSync { .. }) | None => {}
                }
            }
            Action::Submit => {}
            Action::Connect if !self.busy => {
                if !self.connected && self.settings.tun {
                    match multilink_platform::is_elevated() {
                        Ok(false) => {
                            self.request_elevated_restart(true);
                            return;
                        }
                        Err(e) => {
                            self.notify(format!("Не удалось проверить права Windows: {e}"));
                            return;
                        }
                        Ok(true) => {}
                    }
                }
                self.busy = true;
                if self.connected {
                    let _ = self.tx.send(Command::Disconnect);
                } else {
                    let _ = self.tx.send(Command::Connect(self.settings.clone()));
                    self.notify("Запуск и проверка ядра…");
                }
            }
            Action::Connect => {}
            Action::Policy if !self.busy => {
                self.change_routing(self.settings.rules.clone(), self.settings.policy.next());
            }
            Action::ToggleSystemProxy if !self.connected && !self.busy => {
                self.settings.system_proxy = !self.settings.system_proxy;
                self.save();
            }
            Action::ToggleTun if !self.connected && !self.busy => {
                self.settings.tun = !self.settings.tun;
                if self.settings.tun {
                    match multilink_platform::is_elevated() {
                        Ok(false) => {
                            if !self.request_elevated_restart(false) {
                                self.settings.tun = false;
                                self.save();
                            }
                            return;
                        }
                        Err(e) => {
                            self.settings.tun = false;
                            self.notify(format!("Не удалось проверить права Windows: {e}"));
                            return;
                        }
                        Ok(true) => {}
                    }
                }
                self.save();
                self.notify(if self.settings.tun {
                    "TUN выбран · для подключения нужны права администратора"
                } else {
                    "Выбран режим прокси"
                });
            }
            Action::RankServers => {
                self.settings.rank_servers = !self.settings.rank_servers;
                self.offset = 0;
                self.save();
            }
            Action::Toggle(i) if !self.busy => {
                if let Some(node) = self.settings.nodes.get(i) {
                    if self.connected && node.enabled {
                        let enabled_count = self.settings.nodes.iter().filter(|n| n.enabled).count();
                        if enabled_count <= 1 {
                            self.notify("Нельзя отключить единственный активный сервер во время работы VPN");
                            return;
                        }
                    }
                }
                if let Some(node) = self.settings.nodes.get_mut(i) {
                    node.enabled = !node.enabled;
                    self.save();
                    if self.connected {
                        let _ = self.tx.send(Command::ApplySettings(self.settings.clone()));
                    }
                }
            }
            Action::EnableAllServers if !self.busy => {
                if self.settings.nodes.iter().any(|node| !node.enabled) {
                    for node in &mut self.settings.nodes {
                        node.enabled = true;
                    }
                    self.save();
                    if self.connected {
                        let _ = self.tx.send(Command::ApplySettings(self.settings.clone()));
                    }
                    self.notify("Все серверы включены");
                }
            }
            Action::DisableAllServers if !self.busy => {
                if self.connected {
                    self.notify("Нельзя выключить все серверы при активном VPN");
                    return;
                }
                if self.settings.nodes.iter().any(|node| node.enabled) {
                    for node in &mut self.settings.nodes {
                        node.enabled = false;
                    }
                    self.save();
                    self.notify("Все серверы выключены");
                }
            }
            Action::DeleteRule(i) if !self.busy => {
                let mut rules = self.settings.rules.clone();
                let keys = self.rule_members(i);
                rules.retain(|rule| !keys.contains(&multilink_core::rule_key(rule)));
                self.change_routing(rules, self.settings.policy);
            }
            Action::ClearServers if !self.connected && !self.busy => {
                self.modal = Some(Modal::ClearLibrary);
                self.edit.set_text("Очистить библиотеку");
            }
            Action::ToggleAutoconnect => {
                self.settings.autoconnect = !self.settings.autoconnect;
                self.save();
                self.notify(if self.settings.autoconnect {
                    "Автоподключение при старте включено"
                } else {
                    "Автоподключение при старте выключено"
                });
            }
            Action::ToggleRulesOnly => {
                self.settings.rules_only = !self.settings.rules_only;
                self.save();
                if self.connected {
                    let _ = self.tx.send(Command::ApplySettings(self.settings.clone()));
                    self.notify(if self.settings.rules_only {
                        "Режим «Только правила»: прямой доступ для остального трафика"
                    } else {
                        "Стандартный режим: весь трафик через VPN"
                    });
                } else {
                    self.notify(if self.settings.rules_only {
                        "Включён режим «Только правила»"
                    } else {
                        "Выключен режим «Только правила»"
                    });
                }
            }
            Action::OpenSetPin if !self.busy => {
                self.modal = Some(Modal::SetPin);
                self.edit.clear();
                self.edit_scroll = 0.0;
                self.field_focused = true;
                self.reset_caret();
            }
            Action::RemovePin if !self.busy => {
                self.settings.pin_code = None;
                self.pin_unlocked = false;
                self.save();
                self.notify("Защита PIN-кодом отключена");
            }
            Action::LockPin => {
                self.pin_unlocked = false;
                self.page = Page::Dashboard;
                self.notify("Настройки заблокированы PIN-кодом");
            }
            Action::CheckUpdate if !self.busy => {
                if self.checking_update {
                    return;
                }
                self.checking_update = true;
                self.notify("Проверка обновлений на GitHub…");
                let update_tx = self.updates.clone();
                let connected = self.connected;
                std::thread::spawn(move || {
                    let res = multilink_mihomo::check_github_release(
                        env!("CARGO_PKG_VERSION"),
                        connected,
                    );
                    let _ = update_tx.send(Update::UpdateCheckResult(res));
                });
            }
            Action::OpenUpdateLink => {
                let url = self
                    .update_url
                    .as_deref()
                    .unwrap_or("https://github.com/vaalimusic/EvertyMultiLink/releases/latest");
                if multilink_platform::open_project_site(url).is_err() {
                    self.notify(format!("Не удалось открыть браузер: {url}"));
                }
            }
            _ => self.notify("Отключитесь перед изменением конфигурации"),
        }
        self.sync_tray();
    }
    fn sync_tray(&self) {
        #[cfg(windows)]
        {
            if let Some(item) = &self.tray_connect {
                item.set_enabled(!self.busy);
                item.set_text(
                    i18n::tr(
                        self.language(),
                        if self.busy {
                            "Подождите…"
                        } else if self.connected {
                            "Отключиться"
                        } else {
                            "Подключиться"
                        },
                    )
                    .as_ref(),
                );
            }
            for (item, label) in &self.tray_labels {
                item.set_text(i18n::tr(self.language(), label).as_ref());
            }
            if let Some(item) = &self.tray_rule {
                item.set_enabled(!self.busy);
            }
            if let Some(tray) = &self.tray {
                let state = if !self.connected {
                    0
                } else if self.settings.tun {
                    2
                } else {
                    1
                };
                if self.tray_visual.get() != state {
                    if let Ok(icon) =
                        tray_icon::Icon::from_rgba(tray_pixels(&self.logo, state), 64, 64)
                    {
                        if tray.set_icon(Some(icon)).is_ok() {
                            self.tray_visual.set(state);
                        }
                    }
                }
                let tooltip = i18n::tr(
                    self.language(),
                    if self.busy {
                        "Everty MultiLink · выполняется операция"
                    } else if self.connected && self.settings.tun {
                        "Everty MultiLink · TUN подключён"
                    } else if self.connected {
                        "Everty MultiLink · подключено"
                    } else {
                        "Everty MultiLink · отключено"
                    },
                );
                let _ = tray.set_tooltip(Some(tooltip.as_ref()));
            }
        }
    }
    fn change_routing(&mut self, rules: Vec<String>, policy: multilink_core::Policy) {
        let rules = match multilink_core::normalize_direct_rules(&rules) {
            Ok(rules) => rules,
            Err(error) => {
                self.notify(error);
                return;
            }
        };
        if self.connected {
            self.busy = true;
            let _ = self.tx.send(Command::ApplyRouting(rules, policy));
            self.notify("Применение маршрутизации без отключения…");
        } else {
            self.settings.rules = rules;
            self.settings.policy = policy;
            self.commit_route_notes();
            self.save();
            self.modal = None;
            self.editing_rule = None;
            self.input_rect = None;
            self.input_view = ui::InputView::default();
            self.edit.clear();
            self.notify("Маршрутизация сохранена · применится при подключении");
        }
    }
    fn commit_route_notes(&mut self) {
        if let Some(notes) = self.pending_notes.take() {
            self.settings.rule_notes = notes;
        }
        self.settings.rule_notes.retain(|key, _| {
            self.settings
                .rules
                .iter()
                .any(|rule| multilink_core::rule_key(rule) == *key)
        });
        if let Some(groups) = self.pending_groups.take() {
            self.settings.rule_groups = groups;
        }
        multilink_core::retain_rule_groups(&mut self.settings.rule_groups, &self.settings.rules);
    }
    fn apply_imported_routes(&mut self, replace: bool) {
        let Some(config) = self.imported_routing.clone() else {
            return;
        };
        let config = if replace {
            config
        } else {
            config.merged(&self.settings)
        };
        self.pending_notes = Some(config.notes);
        self.pending_groups = Some(config.groups);
        self.offset = 0;
        self.change_routing(config.rules, config.policy);
    }
    fn submit_single_rule(&mut self, rule: String) {
        self.submit_many_rules(vec![rule]);
    }
    fn rule_members(&self, index: usize) -> Vec<String> {
        let Some(rule) = self.settings.rules.get(index) else {
            return Vec::new();
        };
        let key = multilink_core::rule_key(rule);
        self.settings
            .rule_groups
            .iter()
            .find(|group| group.contains(&key))
            .cloned()
            .unwrap_or_else(|| vec![key])
    }
    fn parse_rule_fields(&self, target: &str) -> Result<Vec<String>, String> {
        let current = self.edit.text().trim();
        let other = self.other_rule_values.trim();
        let (addresses, processes) = if self.modal == Some(Modal::ProcessRule) {
            (other, current)
        } else {
            (current, other)
        };
        let mut rules = Vec::new();
        if !processes.is_empty() {
            rules.extend(multilink_core::rules_from_input(processes, true, target)?);
        }
        if self.antigravity_template
            && !rules
                .iter()
                .any(|r| multilink_core::rule_key(r) == "PROCESS-NAME,antigravity.exe")
        {
            return Err("Оставьте Antigravity.exe в шаблоне или создайте обычное правило".into());
        }
        if !addresses.is_empty() {
            rules.extend(multilink_core::rules_from_input(addresses, false, target)?);
        }
        if rules.is_empty() {
            return Err("Введите домен, IP или имя приложения".into());
        }
        Ok(rules)
    }
    fn submit_many_rules(&mut self, additions: Vec<String>) {
        let keys: Vec<_> = additions
            .iter()
            .map(|rule| multilink_core::rule_key(rule))
            .collect();
        let mut rules = self.settings.rules.clone();
        let mut replaced = self
            .editing_rule
            .map(|i| self.rule_members(i))
            .unwrap_or_default();
        if let Some(index) = self.editing_rule {
            if index >= rules.len() {
                self.notify("Правило больше не существует");
                return;
            }
            for existing in &rules {
                let key = multilink_core::rule_key(existing);
                if !keys.contains(&key) || replaced.contains(&key) {
                    continue;
                }
                if self.antigravity_template
                    && additions.contains(existing)
                    && !self
                        .settings
                        .rule_groups
                        .iter()
                        .any(|group| group.contains(&key))
                {
                    replaced.push(key);
                } else {
                    self.notify("Для этого адреса или приложения уже есть другое правило. Откройте его и измените маршрут.");
                    return;
                }
            }
            let insertion = rules[..index]
                .iter()
                .filter(|rule| !replaced.contains(&multilink_core::rule_key(rule)))
                .count();
            rules.retain(|rule| !replaced.contains(&multilink_core::rule_key(rule)));
            rules.splice(insertion..insertion, additions.iter().cloned());
        } else {
            rules.retain(|existing| !keys.contains(&multilink_core::rule_key(existing)));
            rules.extend(additions.iter().cloned());
        }
        let mut notes = self.settings.rule_notes.clone();
        for old in &replaced {
            notes.remove(old);
        }
        for rule in &additions {
            let key = multilink_core::rule_key(rule);
            notes.remove(&key);
            if !self.note_draft.trim().is_empty() {
                notes.insert(key, self.note_draft.clone());
            }
        }
        self.pending_notes = Some(notes);
        let mut groups = self.settings.rule_groups.clone();
        for group in &mut groups {
            group.retain(|key| !keys.contains(key) && !replaced.contains(key));
        }
        if (matches!(self.modal, Some(Modal::Rule | Modal::ProcessRule)) && keys.len() > 1)
            || self.antigravity_template
        {
            groups.push(keys);
        }
        multilink_core::retain_rule_groups(&mut groups, &rules);
        self.pending_groups = Some(groups);
        self.change_routing(rules, self.settings.policy);
    }
    fn redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    fn rule_targets(&self) -> Vec<(String, String)> {
        let mut options = vec![
            ("DIRECT".into(), "Напрямую".into()),
            ("MULTILINK".into(), "Через VPN".into()),
            ("MODE-LATENCY".into(), "Минимальный пинг (Игры)".into()),
            ("MODE-SMART".into(), "Умный режим (Авто)".into()),
            ("MODE-STABLE".into(), "Стабильный (Резерв)".into()),
            ("MODE-BALANCE".into(), "Балансировка нагрузки".into()),
        ];
        options.extend(
            multilink_core::available_countries(&self.settings.nodes)
                .into_iter()
                .map(|(code, name)| (format!("COUNTRY-{code}"), name.to_owned())),
        );
        let mut servers: Vec<_> = self
            .settings
            .nodes
            .iter()
            .filter(|n| n.enabled)
            .map(|node| (multilink_core::node_country(&node.name), node))
            .collect();
        servers.sort_by(|(a, _), (b, _)| {
            let order = |code: Option<&str>| match code {
                Some("US") => 0,
                Some(_) => 1,
                None => 2,
            };
            order(a.as_deref())
                .cmp(&order(b.as_deref()))
                .then_with(|| a.cmp(b))
        });
        options.extend(servers.into_iter().map(|(code, n)| {
            (
                multilink_core::server_route_target(&n.name),
                code.map_or_else(
                    || format!("Сервер · {}", n.name),
                    |code| format!("{code} · {}", n.name),
                ),
            )
        }));
        if let Some(target) = self
            .editing_rule
            .and_then(|i| self.settings.rules.get(i))
            .and_then(|r| r.split(',').nth(2))
        {
            if !options.iter().any(|(id, _)| id == target) {
                let label =
                    multilink_core::server_route_name(target).unwrap_or_else(|| target.into());
                options.push((target.into(), format!("Недоступен · {label}")));
            }
        }
        options
    }
    fn set_compact(&mut self, compact: bool) {
        self.scroll_bar = None;
        self.scroll_drag = None;
        self.compact = compact;
        self.modal = None;
        self.editing_rule = None;
        self.editing_subscription = None;
        self.pending_notes = None;
        self.note_parent = None;
        self.imported_routing = None;
        self.selecting_target = false;
        self.hits.clear();
        self.disabled_hits.clear();
        self.disabled_hint = None;
        self.hover = None;
        self.pressed = None;
        if let Some(w) = &self.window {
            let (width, height) = if compact {
                (390.0, 500.0)
            } else {
                (1280.0, 850.0)
            };
            w.set_min_inner_size(Some(winit::dpi::LogicalSize::new(
                if compact { 360.0 } else { 1040.0 },
                if compact { 460.0 } else { 740.0 },
            )));
            let _ = w.request_inner_size(winit::dpi::LogicalSize::new(width, height));
            w.request_redraw();
        }
    }
    fn reset_caret(&mut self) {
        self.caret_on = true;
        self.next_caret = Instant::now() + Duration::from_millis(500);
    }
    fn apply_restart_arguments(&mut self, args: &[String]) {
        if args.iter().any(|arg| arg == "--elevated-restart") {
            self.compact = args.iter().any(|arg| arg == "--compact-after-elevation");
            if !self.compact {
                self.page = Page::Settings;
            }
            self.connect_after_restart = args.iter().any(|arg| arg == "--connect-after-elevation");
        }
    }
    fn action_at_pointer(&self) -> Option<Action> {
        self.hits
            .iter()
            .rev()
            .find(|(r, action)| {
                if !r.contains(self.pointer) {
                    return false;
                }
                if self.compact && *action == Action::Connect {
                    let radius = r.size.width.0 / 2.0;
                    let dx = self.pointer.x.0 - (r.left() + radius);
                    let dy = self.pointer.y.0 - (r.top() + radius);
                    dx * dx + dy * dy <= radius * radius
                } else {
                    true
                }
            })
            .map(|(_, a)| *a)
    }
    fn disabled_hint_at_pointer(&self) -> Option<(LogicalRect, String)> {
        self.disabled_hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(self.pointer))
            .cloned()
    }
    fn draw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let scale = DpiScale::new(window.scale_factor()).unwrap_or(DpiScale::ONE);
        if self.modal.is_none() {
            self.input_rect = None;
        }
        let paint = self.paint(
            size.width as f64 / window.scale_factor(),
            size.height as f64 / window.scale_factor(),
        );
        if self
            .renderer
            .render(
                &paint,
                &mut self.text,
                scale,
                PhysicalSize::new(size.width as f64, size.height as f64),
            )
            .is_err()
        {
            return;
        }
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if surface
            .resize(
                NonZeroU32::new(size.width).unwrap(),
                NonZeroU32::new(size.height).unwrap(),
            )
            .is_err()
        {
            return;
        }
        if let Ok(mut buffer) = surface.buffer_mut() {
            for (pixel, rgba) in buffer
                .iter_mut()
                .zip(self.renderer.pixels().chunks_exact(4))
            {
                *pixel = ((rgba[0] as u32) << 16) | ((rgba[1] as u32) << 8) | rgba[2] as u32;
            }
            let _ = buffer.present();
        }
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (width, height, min_width, min_height) = if self.compact {
            (390.0, 500.0, 360.0, 460.0)
        } else {
            (1280.0, 850.0, 1040.0, 740.0)
        };
        let attrs = Window::default_attributes()
            .with_title("Everty MultiLink")
            .with_visible(!self.start_hidden)
            .with_window_icon(winit::window::Icon::from_rgba(self.logo.to_vec(), 64, 64).ok())
            .with_inner_size(winit::dpi::LogicalSize::new(width, height))
            .with_min_inner_size(winit::dpi::LogicalSize::new(min_width, min_height));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Create native window"),
        );
        window.set_ime_allowed(true);
        let context = softbuffer::Context::new(window.clone()).expect("Create display context");
        self.surface = Some(
            softbuffer::Surface::new(&context, window.clone()).expect("Create display surface"),
        );
        self.window = Some(window);
        #[cfg(windows)]
        {
            use tray_icon::{
                menu::{Menu, MenuItem, PredefinedMenuItem},
                Icon, TrayIconBuilder,
            };
            let menu = Menu::new();
            let open = MenuItem::with_id("open", "Открыть MultiLink", true, None);
            let advanced = MenuItem::with_id("advanced", "Расширенное окно", true, None);
            let compact = MenuItem::with_id("compact", "Компактное окно", true, None);
            let connect = MenuItem::with_id("connect", "Подключиться", true, None);
            let rule = MenuItem::with_id("rule", "Правило из буфера…", true, None);
            let exit = MenuItem::with_id("exit", "Выход", true, None);
            let separator = PredefinedMenuItem::separator();
            if menu
                .append_items(&[
                    &connect, &separator, &open, &compact, &advanced, &rule, &exit,
                ])
                .is_ok()
            {
                if self.smoke_tray {
                    let ids: Vec<_> = menu
                        .items()
                        .iter()
                        .map(|item| item.id().as_ref().to_owned())
                        .collect();
                    assert_eq!(ids.first().map(String::as_str), Some("connect"));
                    for required in ["open", "compact", "advanced", "connect", "rule", "exit"] {
                        assert!(
                            ids.iter().any(|id| id == required),
                            "missing tray command {required}"
                        );
                    }
                }
                if let Ok(icon) = Icon::from_rgba(tray_pixels(&self.logo, 0), 64, 64) {
                    if let Ok(tray) = TrayIconBuilder::new()
                        .with_menu(Box::new(menu))
                        .with_menu_on_left_click(false)
                        .with_tooltip("Everty MultiLink · отключено")
                        .with_icon(icon)
                        .build()
                    {
                        self.tray = Some(tray);
                        self.tray_labels = vec![
                            (open, "Открыть MultiLink"),
                            (compact, "Компактное окно"),
                            (advanced, "Расширенное окно"),
                            (rule.clone(), "Правило из буфера…"),
                            (exit, "Выход"),
                        ];
                        self.tray_connect = Some(connect);
                        self.tray_rule = Some(rule);
                        self.sync_tray();
                    }
                }
            }
            if self.tray.is_none() {
                self.notify("Не удалось создать значок в трее: окно будет закрываться полностью");
                if let Some(w) = &self.window {
                    w.set_visible(true);
                }
            }
            if self.start_hidden && self.tray.is_some() {
                if let Some(w) = &self.window {
                    w.set_visible(false);
                }
            }
            if self.smoke_tray {
                assert!(self.tray.is_some(), "system tray was not created");
                assert_eq!(
                    self.window.as_ref().unwrap().is_visible(),
                    Some(false),
                    "smoke window must stay hidden"
                );
                self.set_compact(false);
                assert!(!self.compact);
                self.set_compact(true);
                assert!(self.compact);
                let id = self.window.as_ref().unwrap().id();
                self.window_event(event_loop, id, WindowEvent::CloseRequested);
                assert!(
                    !event_loop.exiting(),
                    "closing the window must not exit the process"
                );
                assert_eq!(
                    self.window.as_ref().unwrap().is_visible(),
                    Some(false),
                    "close must hide the window"
                );
                println!("Tray creation, compact/expanded modes and close-to-tray: passed");
                event_loop.exit();
                return;
            }
        }
        self.redraw();
        if self.connect_after_restart {
            self.connect_after_restart = false;
            if self.settings.tun {
                self.action(Action::Connect);
            } else {
                self.notify("Не удалось восстановить режим TUN после перезапуска");
            }
            self.redraw();
        } else if !self.initial_autoconnect_checked {
            self.initial_autoconnect_checked = true;
            if self.settings.autoconnect && !self.connected && !self.settings.nodes.is_empty() {
                self.action(Action::Connect);
            }
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_lan_sync();
        self.poll_subscriptions();
        if self.connected
            && !self.busy
            && self.settings.access.as_ref().is_some_and(|a| {
                (a.blocked || a.expires_at <= multilink_core::unix_now())
                    && self.settings.nodes.iter().any(|n| {
                        n.enabled && !n.manually_added && n.subscription_ids == [a.subscription_id]
                    })
            })
        {
            self.busy = true;
            let _ = self.tx.send(Command::Disconnect);
            self.notify("Срок доступа истёк. Подключение останавливается; обновите доступ");
        }
        if self.restart_pending {
            event_loop.exit();
            return;
        }
        #[cfg(windows)]
        {
            while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                match event.id.as_ref() {
                    "open" => {
                        if let Some(w) = &self.window {
                            w.set_minimized(false);
                            w.set_visible(true);
                            w.focus_window();
                        }
                    }
                    "connect" => self.action(Action::Connect),
                    "advanced" | "compact" => {
                        self.set_compact(event.id.as_ref() == "compact");
                        if let Some(w) = &self.window {
                            w.set_minimized(false);
                            w.set_visible(true);
                            w.focus_window();
                        }
                    }
                    "rule" if !self.busy => {
                        self.set_compact(false);
                        if let Some(w) = &self.window {
                            w.set_minimized(false);
                            w.set_visible(true);
                            w.focus_window();
                        }
                        self.page = Page::Routing;
                        self.action(Action::AddRule);
                        self.action(Action::Paste);
                    }
                    "rule" => self.notify("Дождитесь завершения текущей операции"),
                    "exit" => event_loop.exit(),
                    _ => {}
                }
            }
            while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
                if let tray_icon::TrayIconEvent::DoubleClick {
                    button: tray_icon::MouseButton::Left,
                    ..
                } = event
                {
                    self.set_compact(true);
                    if let Some(w) = &self.window {
                        w.set_minimized(false);
                        w.set_visible(true);
                        w.focus_window();
                    }
                } else if matches!(
                    event,
                    tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    }
                ) {
                    if let Some(w) = &self.window {
                        w.set_minimized(false);
                        w.set_visible(true);
                        w.focus_window();
                    }
                }
            }
        }
        if self.restart_pending {
            event_loop.exit();
            return;
        }
        if self.modal.is_some() && self.field_focused && !self.busy {
            if Instant::now() >= self.next_caret {
                self.caret_on = !self.caret_on;
                self.next_caret = Instant::now() + Duration::from_millis(500);
                self.redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                self.next_caret
                    .min(Instant::now() + Duration::from_millis(200)),
            ));
        } else {
            let visible = self
                .window
                .as_ref()
                .is_some_and(|w| w.is_visible() == Some(true) && w.is_minimized() != Some(true));
            let animate = self.compact && self.connected && self.settings.tun && visible;
            if Instant::now() >= self.next_visual {
                if self.compact && self.connected && visible {
                    self.redraw();
                }
                self.next_visual =
                    Instant::now() + Duration::from_millis(if animate { 33 } else { 1_000 });
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                self.next_visual
                    .min(Instant::now() + Duration::from_millis(200)),
            ));
        }
    }
    fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
        self.drain();
        self.redraw();
    }
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                #[cfg(windows)]
                if self.tray.is_some() {
                    if let Some(w) = &self.window {
                        w.set_visible(false);
                    }
                    return;
                }
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor())
                    .unwrap_or(1.0);
                let old_field = self.input_rect.is_some_and(|r| r.contains(self.pointer));
                self.pointer = LogicalPoint::new(position.x / scale, position.y / scale);
                if let (Some(grab), Some(bar)) = (self.scroll_drag, self.scroll_bar) {
                    self.offset = bar.offset_at(self.pointer.y.0 - grab);
                    self.redraw();
                    return;
                }
                let old_disabled = self.disabled_hint.clone();
                let disabled_at_pointer = self.disabled_hint_at_pointer();
                self.disabled_hint = disabled_at_pointer.as_ref().map(|(_, hint)| hint.clone());
                let hover = self.action_at_pointer();
                if hover != self.hover || self.disabled_hint != old_disabled {
                    self.hover = hover;
                    self.redraw();
                }
                let field = self.input_rect.is_some_and(|r| r.contains(self.pointer));
                if field != old_field {
                    self.redraw();
                }
                if let Some(w) = &self.window {
                    w.set_cursor(if field {
                        winit::window::CursorIcon::Text
                    } else if hover.is_some() {
                        winit::window::CursorIcon::Pointer
                    } else if self.disabled_hint.is_some() {
                        winit::window::CursorIcon::NotAllowed
                    } else {
                        winit::window::CursorIcon::Default
                    });
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.hover = None;
                self.disabled_hint = None;
                self.pressed = None;
                self.redraw();
            }
            WindowEvent::Focused(false) => {
                self.scroll_drag = None;
                self.pressed = None;
                self.redraw();
            }
            WindowEvent::MouseInput {
                state,
                button: WMouse::Left,
                ..
            } => {
                self.redraw();
                match state {
                    ElementState::Pressed => {
                        if let Some(bar) = self
                            .scroll_bar
                            .filter(|b| b.track.inset(Insets::all(-5.0)).contains(self.pointer))
                        {
                            let grab = if bar.thumb.contains(self.pointer) {
                                self.pointer.y.0 - bar.thumb.top()
                            } else {
                                bar.thumb.size.height.0 / 2.0
                            };
                            self.scroll_drag = Some(grab);
                            self.offset = bar.offset_at(self.pointer.y.0 - grab);
                            self.pressed = None;
                            return;
                        }
                        self.pressed = self.action_at_pointer();
                        if let Some(rect) = self.input_rect.filter(|r| r.contains(self.pointer)) {
                            self.field_focused = true;
                            let index = self
                                .input_view
                                .index_at(self.pointer.x.0 - rect.left() - 14.0 + self.edit_scroll);
                            self.edit.set_caret(index, self.modifiers.shift_key());
                            self.reset_caret();
                        } else if self.modal.is_some() && self.pressed.is_none() {
                            self.field_focused = false;
                        }
                    }
                    ElementState::Released => {
                        if self.scroll_drag.take().is_some() {
                            return;
                        }
                        let pressed = self.pressed.take();
                        if pressed.is_some() && pressed == self.action_at_pointer() {
                            self.action(pressed.unwrap());
                        }
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::MouseWheel { delta, .. } if self.modal.is_none() => {
                self.redraw();
                let down = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y < 0.0,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y < 0.0,
                };
                let max = self.scroll_bar.map(|b| b.max).unwrap_or(0);
                if down {
                    self.offset = (self.offset + 1).min(max);
                } else {
                    self.offset = self.offset.saturating_sub(1);
                }
            }
            WindowEvent::Ime(Ime::Commit(text))
                if self.modal.is_some()
                    && self.modal != Some(Modal::RuRule)
                    && !self.selecting_target
                    && !self.busy =>
            {
                self.edit.insert(&text);
                self.reset_caret();
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.redraw();
                self.reset_caret();
                if self.modal.is_none()
                    && self.compact
                    && matches!(
                        event.logical_key,
                        WKey::Named(NamedKey::Enter | NamedKey::Space)
                    )
                {
                    self.action(Action::Connect);
                    return;
                }
                if event.logical_key == WKey::Named(NamedKey::Escape) {
                    if self.selecting_target {
                        self.selecting_target = false;
                        return;
                    }
                    self.action(Action::Close);
                    return;
                }
                if self.modal.is_none() || self.busy {
                    return;
                }
                if self.selecting_target {
                    match event.logical_key {
                        WKey::Named(NamedKey::ArrowDown) => {
                            self.rule_target = (self.rule_target + 1) % self.rule_targets().len();
                            self.target_page = self.rule_target / 18;
                        }
                        WKey::Named(NamedKey::ArrowUp) => {
                            self.rule_target = (self.rule_target + self.rule_targets().len() - 1)
                                % self.rule_targets().len();
                            self.target_page = self.rule_target / 18;
                        }
                        WKey::Named(NamedKey::Enter) => self.selecting_target = false,
                        _ => {}
                    }
                    return;
                }
                if self.modal == Some(Modal::RuRule) {
                    if event.logical_key == WKey::Named(NamedKey::Enter) {
                        self.action(Action::Submit);
                    }
                    return;
                }
                if event.logical_key == WKey::Named(NamedKey::Tab) {
                    self.field_focused = true;
                    return;
                }
                if self.modifiers.control_key() {
                    match &event.logical_key {
                        WKey::Character(c) if c.eq_ignore_ascii_case("v") => {
                            self.action(Action::Paste);
                            return;
                        }
                        WKey::Character(c) if c.eq_ignore_ascii_case("a") => {
                            self.edit.select_all();
                            return;
                        }
                        WKey::Character(c)
                            if c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("x") =>
                        {
                            if let Some(selected) = self.edit.selected_text().map(str::to_owned) {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    if clipboard.set_text(selected).is_ok()
                                        && c.eq_ignore_ascii_case("x")
                                    {
                                        self.edit.backspace(false);
                                    }
                                }
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                if event.logical_key == WKey::Named(NamedKey::Enter) {
                    self.action(Action::Submit);
                    return;
                }
                if !self.field_focused {
                    return;
                }
                if let Some(key) =
                    everty_ui::platform::winit::key(&event.logical_key, event.physical_key)
                {
                    let handled = self
                        .edit
                        .handle_key(key, everty_ui::platform::winit::modifiers(self.modifiers));
                    if !handled && !self.modifiers.control_key() && !self.modifiers.alt_key() {
                        if let Some(text) = event.text {
                            self.edit.insert(&text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
fn main() {
    if multilink_platform::handle_watchdog_args() {
        return;
    }
    let args: Vec<_> = std::env::args().collect();
    #[cfg(windows)]
    if args.iter().any(|arg| arg == "--smoke-watchdog") {
        let before = multilink_platform::proxy::ProxySettings::read().expect("read original proxy");
        let _listener = std::net::TcpListener::bind(("127.0.0.1", multilink_mihomo::PORT))
            .expect("test requires a free proxy port");
        let mut guard = multilink_platform::SystemProxyGuard::enable(multilink_mihomo::PORT)
            .expect("start watchdog from GUI executable");
        assert!(multilink_platform::proxy::ProxySettings::read()
            .unwrap()
            .server
            .contains("127.0.0.1:17890"));
        guard
            .restore()
            .expect("restore proxy after watchdog smoke test");
        assert!(
            multilink_platform::proxy::ProxySettings::read().unwrap() == before,
            "original proxy must be restored"
        );
        println!("GUI executable watchdog handshake and proxy restoration: passed");
        return;
    }
    let offline = args.iter().any(|s| s == "--preview" || s == "--bench-ui");
    #[cfg(windows)]
    let _instance = if !offline && !args.iter().any(|s| s == "--smoke-tray") {
        let acquired = if args.iter().any(|s| s == "--elevated-restart") {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                match multilink_platform::instance::acquire() {
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(100))
                    }
                    result => break result,
                }
            }
        } else {
            multilink_platform::instance::acquire()
        };
        match acquired {
            Ok(Some(instance)) => Some(instance),
            Ok(None) => {
                if !args.iter().any(|s| s == "--start-hidden") {
                    for _ in 0..40 {
                        if multilink_platform::instance::show_existing() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                return;
            }
            Err(_) => None,
        }
    } else {
        None
    };
    let event_loop = if offline {
        None
    } else {
        Some(
            EventLoop::<()>::with_user_event()
                .build()
                .expect("Create event loop"),
        )
    };
    let mut app = App::new(event_loop.as_ref().map(EventLoop::create_proxy));
    app.apply_restart_arguments(&args);
    if offline {
        if let Some(index) = args.iter().position(|a| a == "--language") {
            if let Some(language) = args.get(index + 1) {
                app.settings.language = language.clone();
            }
        }
    }
    if args.iter().any(|s| s == "--bench-ui") {
        app.compact = false;
        for modal in [None, Some(Modal::Import)] {
            app.modal = modal.clone();
            app.edit
                .set_text("https://provider.example/subscription/test");
            let mut times = Vec::new();
            for i in 0..35 {
                let start = Instant::now();
                let paint = app.paint(1280.0, 850.0);
                app.renderer
                    .render(
                        &paint,
                        &mut app.text,
                        DpiScale::ONE,
                        PhysicalSize::new(1280.0, 850.0),
                    )
                    .unwrap();
                if i >= 5 {
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{}: median {:.2} ms, p95 {:.2} ms",
                if modal.is_some() {
                    "input"
                } else {
                    "dashboard"
                },
                times[15],
                times[28]
            );
        }
        return;
    }
    if let Some(i) = args.iter().position(|s| s == "--preview") {
        app.compact = false;
        if let Some(page) = args.get(i + 2) {
            app.compact = page == "compact" || page == "compact-tun";
            if page == "compact-tun" {
                app.connected = true;
                app.settings.tun = true;
                app.started = Some(Instant::now() - Duration::from_secs(65));
                app.notice = "Подключено · TUN-адаптер активен".into();
            }
            app.page = match page.as_str() {
                "servers" => Page::Servers,
                "routing" => Page::Routing,
                "settings" => Page::Settings,
                "language" => Page::Settings,
                "about" => Page::About,
                "access" => Page::Access,
                "help" => Page::Help,
                "subscriptions" => Page::Subscriptions,
                "statistics" => Page::Statistics,
                _ => Page::Dashboard,
            };
            if ["picker", "rule", "ru-modal"].contains(&page.as_str()) {
                app.page = Page::Routing;
                app.modal = Some(if page == "ru-modal" {
                    Modal::RuRule
                } else {
                    Modal::Rule
                });
                app.selecting_target = page == "picker";
                app.target_page = 1;
                app.notice = "Выберите маршрут для правила".into();
                if page == "ru-modal" {
                    app.edit.set_text("Российские сайты и домены");
                }
                let template = multilink_core::parse_subscription(
                    "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#Demo",
                )
                .unwrap()
                .remove(0);
                app.settings.nodes = multilink_core::COUNTRIES
                    .iter()
                    .map(|(code, _, _)| {
                        let mut node = template.clone();
                        node.name = format!("{code}-01");
                        node
                    })
                    .collect();
            }
            if ["editor-long", "rule-note", "route-import", "ranked-servers"]
                .contains(&page.as_str())
            {
                app.settings = Settings::default();
                app.settings.nodes = multilink_core::parse_subscription(
                    "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#Demo",
                )
                .unwrap();
                app.settings.nodes[0].name = "🇩🇪 Germany · Очень длинное название сервера для проверки переполнения окна · Frankfurt premium 01".into();
                app.settings.rules = vec![format!(
                    "DOMAIN-SUFFIX,example.com,{}",
                    app.settings.nodes[0].name
                )];
                app.settings.rule_notes.insert(
                    "DOMAIN-SUFFIX,example.com".into(),
                    "Основной сайт и дополнительный домен приложения".into(),
                );
                app.page = Page::Routing;
                app.action(Action::EditRule(0));
                if page == "rule-note" {
                    app.action(Action::RuleNote);
                }
                if page == "route-import" {
                    app.imported_routing =
                        Some(multilink_core::RoutingConfig::from_settings(&app.settings));
                    app.modal = Some(Modal::RouteImport);
                }
                if page == "ranked-servers" {
                    app.modal = None;
                    app.page = Page::Servers;
                    let mut second = app.settings.nodes[0].clone();
                    second.name = "🇺🇸 US · sample".into();
                    app.settings.nodes.push(second);
                    for (i, node) in app.settings.nodes.iter_mut().enumerate() {
                        for sample in 0..8 {
                            node.quality
                                .observe(&sample.to_string(), Some(80 + i as u64 * 700));
                        }
                    }
                    app.settings.rank_servers = true;
                }
                app.notice = "Тестовые данные · без подключения к сети".into();
            }
            if [
                "subscriptions-demo",
                "subscription-url",
                "subscription-name",
                "subscription-delete",
                "routing-defaults",
                "overview-demo",
            ]
            .contains(&page.as_str())
            {
                app.settings = Settings::default();
                multilink_core::initialize_ru_routes(&mut app.settings);
                app.page = Page::Subscriptions;
                let nodes = multilink_core::parse_subscription(
                    "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#DE-demo",
                )
                .unwrap();
                let id = multilink_core::update_subscription(
                    &mut app.settings,
                    None,
                    "https://subscription.example.invalid/demo-private-key",
                    nodes,
                )
                .unwrap();
                app.settings.subscriptions[0].name = "Основная подписка · Everty".into();
                match page.as_str() {
                    "subscription-url" => app.action(Action::EditSubscriptionUrl(id)),
                    "subscription-name" => app.action(Action::RenameSubscription(id)),
                    "subscription-delete" => app.action(Action::DeleteSubscription(id)),
                    "routing-defaults" => app.page = Page::Routing,
                    "overview-demo" => app.page = Page::Dashboard,
                    _ => {}
                }
                app.notice = "Тестовые данные · без подключения к сети".into();
            }
        }
        if let Some(page) = args.get(i + 2) {
            if [
                "access-code",
                "access-active",
                "access-expired",
                "access-pending",
                "routing-hint",
            ]
            .contains(&page.as_str())
            {
                app.settings = Settings::default();
                app.compact = false;
                app.page = Page::Access;
                if matches!(page.as_str(), "access-active" | "access-expired") {
                    let mut doc = multilink_core::ProfileDocument::decode(include_str!(
                        "../../multilink-core/fixtures/profile.example.everty.json"
                    ))
                    .unwrap();
                    doc.name = "Личный доступ Everty".into();
                    doc.expires_at = Some(multilink_core::unix_now() + 90 * 86400);
                    multilink_core::install_access(&mut app.settings, "a".repeat(64), doc).unwrap();
                    if page == "access-expired" {
                        app.settings.access.as_mut().unwrap().expires_at = 1;
                    }
                }
                if page == "access-code" {
                    app.action(Action::ActivateAccess);
                    app.edit.set_text("AB7K9X");
                }
                if page == "access-pending" {
                    app.settings.pending_activation = Some(multilink_core::PendingActivation {
                        code: "AB7K9X".into(),
                        nonce: "b".repeat(64),
                    });
                }
                if page == "routing-hint" {
                    multilink_core::initialize_ru_routes(&mut app.settings);
                    app.page = Page::Routing;
                    app.hover = Some(Action::ImportRoutes);
                }
                app.notice = "Тестовые данные · без подключения к сети".into();
            }
        }
        if let Some(page) = args.get(i + 2) {
            if matches!(page.as_str(), "scroll-servers" | "scroll-routing") {
                app.settings = Settings::default();
                if let Some(index) = args.iter().position(|a| a == "--language") {
                    if let Some(language) = args.get(index + 1) {
                        app.settings.language = language.clone();
                    }
                }
                let template = multilink_core::parse_subscription(
                    "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#DE-demo",
                )
                .unwrap()
                .remove(0);
                app.settings.nodes = (0..40)
                    .map(|i| {
                        let mut n = template.clone();
                        n.name = format!("DE-demo-{:02}", i + 1);
                        n
                    })
                    .collect();
                app.settings.rules = (0..40)
                    .map(|i| format!("DOMAIN-SUFFIX,site-{i}.example.invalid,MULTILINK"))
                    .collect();
                app.page = if page == "scroll-servers" {
                    Page::Servers
                } else {
                    Page::Routing
                };
                app.notice = "Тестовые данные · без подключения к сети".into();
            }
        }
        if let Some(index) = args.iter().position(|a| a == "--section") {
            app.offset = args
                .get(index + 1)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
        }
        // Fixtures may reset Settings; apply the preview language last.
        if args.get(i + 2).is_some_and(|p| p == "language") {
            app.modal = Some(Modal::Language);
        }
        if let Some(index) = args.iter().position(|a| a == "--language") {
            if let Some(language) = args.get(index + 1) {
                app.settings.language = language.clone();
            }
        }
        let (width, height) = if app.compact {
            (390, 500)
        } else if args.iter().any(|a| a == "--small") {
            (1040, 740)
        } else {
            (1280, 850)
        };
        let paint = app.paint(width as f64, height as f64);
        app.renderer
            .render(
                &paint,
                &mut app.text,
                DpiScale::ONE,
                PhysicalSize::new(width as f64, height as f64),
            )
            .unwrap();
        image::save_buffer(
            args.get(i + 1).expect("--preview PATH"),
            app.renderer.pixels(),
            width,
            height,
            image::ColorType::Rgba8,
        )
        .unwrap();
        return;
    }
    event_loop
        .expect("Create event loop")
        .run_app(&mut app)
        .expect("Run application");
}

#[cfg(windows)]
fn autostart_enabled() -> std::io::Result<bool> {
    use winreg::{enums::*, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run = match hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") {
        Ok(k) => k,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    autostart_at(&run, &std::env::current_exe()?)
}
#[cfg(windows)]
fn autostart_at(run: &winreg::RegKey, exe: &std::path::Path) -> std::io::Result<bool> {
    let value: String = match run.get_value("Everty MultiLink") {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let expected = format!("\"{}\" --start-hidden", exe.display());
    Ok(value.eq_ignore_ascii_case(&expected))
}
#[cfg(windows)]
fn set_autostart(enabled: bool) -> std::io::Result<()> {
    use winreg::{enums::*, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    set_autostart_at(&run, &std::env::current_exe()?, enabled)
}
#[cfg(windows)]
fn set_autostart_at(
    run: &winreg::RegKey,
    exe: &std::path::Path,
    enabled: bool,
) -> std::io::Result<()> {
    if enabled {
        run.set_value(
            "Everty MultiLink",
            &format!("\"{}\" --start-hidden", exe.display()),
        )
    } else {
        if !autostart_at(run, exe)? {
            return match run.get_raw_value("Everty MultiLink") {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
                Ok(_) => Err(std::io::Error::other("Запись автозапуска изменилась")),
            };
        }
        match run.delete_value("Everty MultiLink") {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
#[cfg(not(windows))]
fn autostart_enabled() -> std::io::Result<bool> {
    Ok(false)
}
#[cfg(not(windows))]
fn set_autostart(_: bool) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "Автозапуск поддерживается только в Windows",
    ))
}

#[cfg(all(test, windows))]
mod autostart_tests {
    use super::*;
    use winreg::{enums::*, RegKey};
    #[test]
    fn autostart_uses_quoted_executable_and_preserves_foreign_changes() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key_path = format!(
            "Software\\Everty\\MultiLink\\Tests\\autostart-{}",
            std::process::id()
        );
        let (run, _) = hkcu.create_subkey(&key_path).unwrap();
        let exe = std::path::Path::new(r"C:\Program Files\Everty MultiLink\app.exe");
        set_autostart_at(&run, exe, true).unwrap();
        let value: String = run.get_value("Everty MultiLink").unwrap();
        assert_eq!(
            value,
            r#""C:\Program Files\Everty MultiLink\app.exe" --start-hidden"#
        );
        assert!(autostart_at(&run, exe).unwrap());
        set_autostart_at(&run, exe, false).unwrap();
        assert!(!autostart_at(&run, exe).unwrap());
        run.set_value("Everty MultiLink", &"other-installation")
            .unwrap();
        assert!(set_autostart_at(&run, exe, false).is_err());
        assert_eq!(
            run.get_value::<String, _>("Everty MultiLink").unwrap(),
            "other-installation"
        );
        drop(run);
        hkcu.delete_subkey(&key_path).unwrap();
    }
}

#[cfg(test)]
mod routing_workflow_tests {
    use super::*;
    use multilink_core::Policy;
    #[test]
    fn elevated_restart_preserves_requested_window_mode_and_autoconnect() {
        let mut app = App::new(None);
        let compact: Vec<_> = multilink_platform::elevation_arguments(true, true)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        app.apply_restart_arguments(&compact);
        assert!(app.compact);
        assert!(app.connect_after_restart);
        let expanded: Vec<_> = multilink_platform::elevation_arguments(false, false)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        app.apply_restart_arguments(&expanded);
        assert!(!app.compact);
        assert!(!app.connect_after_restart);
        assert_eq!(app.page, Page::Settings);
    }
    #[test]
    fn activation_retries_reuse_saved_nonce_and_commit_only_on_success() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        app.connected = false;
        app.action(Action::ActivateAccess);
        app.edit.set_text("ab7-k9x");
        app.action(Action::Submit);
        let pending = match received.recv().unwrap() {
            Command::Activate(p, s) => {
                assert_eq!(s.pending_activation.unwrap().nonce, p.nonce);
                p
            }
            _ => panic!("activation"),
        };
        assert!(app.settings.access.is_none());
        updates
            .send(Update::OperationError("simulated lost response".into()))
            .unwrap();
        app.drain();
        app.action(Action::Submit);
        match received.recv().unwrap() {
            Command::Activate(p, _) => {
                assert_eq!(p.nonce, pending.nonce);
                assert_eq!(p.code, "AB7K9X");
            }
            _ => panic!("retry"),
        };
        let mut doc = multilink_core::ProfileDocument::decode(include_str!(
            "../../multilink-core/fixtures/profile.example.everty.json"
        ))
        .unwrap();
        doc.expires_at = Some(multilink_core::unix_now() + 86400);
        updates
            .send(Update::AccessReady("a".repeat(64), doc))
            .unwrap();
        app.drain();
        assert!(app.settings.pending_activation.is_none());
        assert!(app.settings.access.is_some());
        assert!(app.modal.is_none());
    }
    #[test]
    fn subscription_url_changes_only_after_successful_download() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        let text = "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#DE-test";
        let id = multilink_core::update_subscription(
            &mut app.settings,
            None,
            "https://old.invalid/private",
            multilink_core::parse_subscription(text).unwrap(),
        )
        .unwrap();
        app.settings.nodes[0].enabled = false;
        app.action(Action::EditSubscriptionUrl(id));
        app.edit.set_text("https://new.invalid/private");
        app.action(Action::Submit);
        assert!(
            matches!(received.recv().unwrap(), Command::Import(_, Some(source)) if source == id)
        );
        assert_eq!(
            app.settings.subscriptions[0].url,
            "https://old.invalid/private"
        );
        updates
            .send(Update::OperationError("test download failed".into()))
            .unwrap();
        app.drain();
        assert!(!app.busy);
        assert!(app.modal == Some(Modal::SubscriptionUrl));
        assert_eq!(
            app.settings.subscriptions[0].url,
            "https://old.invalid/private"
        );
        app.action(Action::Submit);
        assert!(
            matches!(received.recv().unwrap(), Command::Import(_, Some(source)) if source == id)
        );
        updates
            .send(Update::Imported(
                multilink_core::import_subscription(text).unwrap(),
                Some("https://new.invalid/private".into()),
                Some(id),
            ))
            .unwrap();
        app.drain();
        assert_eq!(app.settings.subscriptions.len(), 1);
        assert_eq!(
            app.settings.subscriptions[0].url,
            "https://new.invalid/private"
        );
        assert!(!app.settings.nodes[0].enabled);
        assert!(app.modal.is_none());
    }

    #[test]
    fn antigravity_migrates_a_legacy_rule_and_edits_mixed_conditions_as_one_group() {
        let mut app = App::new(None);
        let (tx, _commands) = mpsc::channel();
        app.tx = tx;
        app.settings = Settings::default();
        app.settings.rules = vec!["PROCESS-NAME,antigravity.exe,MULTILINK".into()];
        app.action(Action::AddAntigravity);
        app.edit.set_text("Antigravity.exe, helper.exe");
        app.action(Action::RuleField(false));
        assert_eq!(app.edit.text(), "googleapis.com");
        app.edit.set_text("googleapis.com, 192.0.2.4");
        app.action(Action::Submit);
        assert_eq!(app.settings.rules.len(), 4);
        assert_eq!(app.settings.rule_groups.len(), 1);
        let exported = multilink_core::RoutingConfig::from_settings(&app.settings)
            .encode()
            .unwrap();
        let roundtrip = multilink_core::RoutingConfig::decode(&exported).unwrap();
        assert_eq!(roundtrip.groups, app.settings.rule_groups);
        app.action(Action::AddAntigravity);
        app.action(Action::RuleField(false));
        assert!(app.edit.text().contains("192.0.2.4"));
        app.edit.set_text("googleapis.com");
        app.action(Action::Submit);
        assert_eq!(app.settings.rules.len(), 3);
        app.action(Action::DeleteRule(0));
        assert!(app.settings.rules.is_empty() && app.settings.rule_groups.is_empty());
    }

    #[test]
    fn background_refresh_stages_new_nodes_without_interrupting_connection() {
        let mut app = App::new(None);
        let (sender, commands) = mpsc::channel();
        app.tx = sender;
        app.settings = Settings::default();
        app.modal = None;
        app.connected = true;
        let url = "https://subscription.invalid/link";
        let old = "vless://00000000-0000-4000-8000-000000000001@old.invalid:443#US-old";
        let fresh = "vless://00000000-0000-4000-8000-000000000002@new.invalid:443#US-new";
        let id = multilink_core::update_subscription(
            &mut app.settings,
            None,
            url,
            multilink_core::parse_subscription(old).unwrap(),
        )
        .unwrap();
        app.refreshing_subscription = Some(id);
        app.pending_subscription = Some((id, url.into(), Err("offline".into())));
        app.poll_subscriptions();
        assert_eq!(app.settings.nodes[0].name, "US-old");
        assert!(app.subscription_errors.contains(&id));
        assert!(commands.try_recv().is_err());
        app.refreshing_subscription = Some(id);
        app.pending_subscription =
            Some((id, url.into(), multilink_core::import_subscription(fresh)));
        app.poll_subscriptions();
        assert_eq!(app.settings.nodes[0].name, "US-new");
        assert!(app.connected && app.subscription_changes_pending);
        assert!(!app.subscription_errors.contains(&id));
        assert!(matches!(commands.try_recv(), Ok(Command::Save(_))));
        assert!(commands.try_recv().is_err());
    }
    #[test]
    fn imported_routes_and_notes_survive_failed_apply_without_partial_changes() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        app.settings.rules = vec!["DOMAIN-SUFFIX,old.example,DIRECT".into()];
        app.settings
            .rule_notes
            .insert("DOMAIN-SUFFIX,old.example".into(), "old note".into());
        let mut incoming = multilink_core::RoutingConfig::from_settings(&Settings::default());
        incoming.policy = Policy::Smart;
        incoming.rules = vec!["DOMAIN-SUFFIX,new.example,MULTILINK".into()];
        incoming
            .notes
            .insert("DOMAIN-SUFFIX,new.example".into(), "new note".into());
        app.imported_routing = Some(incoming);
        app.modal = Some(Modal::RouteImport);
        app.connected = true;
        app.apply_imported_routes(true);
        assert!(matches!(
            received.recv().unwrap(),
            Command::ApplyRouting(_, Policy::Smart)
        ));
        updates
            .send(Update::RoutingFailed("test rejection".into()))
            .unwrap();
        app.drain();
        assert_eq!(app.settings.rules, ["DOMAIN-SUFFIX,old.example,DIRECT"]);
        assert_eq!(app.settings.rule_notes.len(), 1);
        assert_eq!(
            app.settings.rule_notes["DOMAIN-SUFFIX,old.example"],
            "old note"
        );
        assert!(app.settings.policy == Policy::Balance);
        app.apply_imported_routes(true);
        let (rules, policy) = match received.recv().unwrap() {
            Command::ApplyRouting(rules, policy) => (rules, policy),
            _ => panic!("expected imported routes"),
        };
        updates
            .send(Update::RoutingApplied(
                rules,
                policy,
                multilink_mihomo::RoutingChange::default(),
            ))
            .unwrap();
        app.drain();
        assert_eq!(app.settings.rules, ["DOMAIN-SUFFIX,new.example,MULTILINK"]);
        assert_eq!(app.settings.rule_notes.len(), 1);
        assert_eq!(
            app.settings.rule_notes["DOMAIN-SUFFIX,new.example"],
            "new note"
        );
        assert!(app.settings.policy == Policy::Smart);
    }
    #[test]
    fn batch_edit_and_note_commit_together_after_core_acknowledgement() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        app.settings.rules = vec!["PROCESS-NAME,old.exe,DIRECT".into()];
        app.settings
            .rule_notes
            .insert("PROCESS-NAME,old.exe".into(), "old".into());
        app.connected = true;
        app.action(Action::EditRule(0));
        app.edit.set_text("one.exe, helper.exe");
        app.action(Action::RuleNote);
        assert!(app.modal == Some(Modal::Note));
        app.edit.set_text("main process and helper");
        app.action(Action::Submit);
        assert_eq!(app.edit.text(), "one.exe, helper.exe");
        app.action(Action::Submit);
        let (rules, policy) = match received.recv().unwrap() {
            Command::ApplyRouting(r, p) => (r, p),
            _ => panic!("expected routing"),
        };
        assert_eq!(app.settings.rules, ["PROCESS-NAME,old.exe,DIRECT"]);
        assert_eq!(app.settings.rule_notes["PROCESS-NAME,old.exe"], "old");
        updates
            .send(Update::RoutingApplied(
                rules,
                policy,
                multilink_mihomo::RoutingChange::default(),
            ))
            .unwrap();
        app.drain();
        assert_eq!(app.settings.rules.len(), 2);
        assert_eq!(
            app.settings.rule_notes["PROCESS-NAME,helper.exe"],
            "main process and helper"
        );
        assert!(!app.settings.rule_notes.contains_key("PROCESS-NAME,old.exe"));
    }
    #[test]
    fn system_proxy_can_be_selected_alongside_tun_and_reported_as_active() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        app.settings.tun = true;
        app.settings.system_proxy = false;
        app.compact = false;
        app.page = Page::Settings;
        app.paint(1040.0, 740.0);
        assert!(app
            .hits
            .iter()
            .any(|(_, a)| *a == Action::ToggleSystemProxy));
        app.action(Action::ToggleSystemProxy);
        assert!(app.settings.tun && app.settings.system_proxy);
        assert!(matches!(received.try_recv(), Ok(Command::Save(_))));
        updates.send(Update::Connected(true)).unwrap();
        app.drain();
        assert!(app.connected && app.system_proxy_active);
        assert!(app.notice.contains("TUN и системный прокси"));
        app.paint(1040.0, 740.0);
        assert!(!app
            .hits
            .iter()
            .any(|(_, a)| *a == Action::ToggleSystemProxy));
    }
    #[cfg(windows)]
    #[test]
    fn tray_status_colors_preserve_logo_transparency() {
        let logo = [255, 255, 255, 255, 40, 60, 80, 0];
        let gray = tray_pixels(&logo, 0);
        let proxy = tray_pixels(&logo, 1);
        let tun = tray_pixels(&logo, 2);
        for pixels in [&gray, &proxy, &tun] {
            assert_eq!(pixels[3], 255);
            assert_eq!(pixels[7], 0);
        }
        assert!(gray[2] - gray[0] < 20);
        assert!(proxy[1] > proxy[0] && proxy[1] > proxy[2]);
        assert!(tun[0] > tun[1] && tun[1] > tun[2]);
    }
    #[test]
    fn editing_rules_keeps_original_until_success_and_does_not_overwrite_another_rule() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        app.settings.rules = vec![
            "DOMAIN-SUFFIX,old.example,DIRECT".into(),
            "DOMAIN-SUFFIX,other.example,MULTILINK".into(),
            "RULE-SET,RU-SITES,DIRECT".into(),
            "DOMAIN-SUFFIX,ru,DIRECT".into(),
        ];
        app.connected = true;
        app.action(Action::EditRule(0));
        assert_eq!(app.edit.text(), "old.example");
        app.edit.set_text("other.example");
        app.action(Action::Submit);
        assert!(!app.busy);
        assert!(received.try_recv().is_err());
        app.edit.set_text("new.example");
        app.rule_target = 1;
        app.action(Action::Submit);
        let (rules, policy) = match received.recv().unwrap() {
            Command::ApplyRouting(rules, policy) => (rules, policy),
            _ => panic!("expected route update"),
        };
        assert!(app
            .settings
            .rules
            .contains(&"DOMAIN-SUFFIX,old.example,DIRECT".into()));
        assert!(rules.contains(&"DOMAIN-SUFFIX,new.example,MULTILINK".into()));
        updates.send(Update::RoutingFailed("error".into())).unwrap();
        app.drain();
        assert!(app
            .settings
            .rules
            .contains(&"DOMAIN-SUFFIX,old.example,DIRECT".into()));
        app.action(Action::Submit);
        assert!(matches!(
            received.recv().unwrap(),
            Command::ApplyRouting(_, _)
        ));
        updates
            .send(Update::RoutingApplied(
                rules,
                policy,
                multilink_mihomo::RoutingChange::default(),
            ))
            .unwrap();
        app.drain();
        assert!(matches!(received.recv().unwrap(), Command::Save(_)));
        assert!(app
            .settings
            .rules
            .contains(&"DOMAIN-SUFFIX,new.example,MULTILINK".into()));
        let ru = app
            .settings
            .rules
            .iter()
            .position(|r| r.starts_with("RULE-SET,"))
            .unwrap();
        app.action(Action::EditRule(ru));
        app.rule_target = 1;
        app.action(Action::Submit);
        let rules = match received.recv().unwrap() {
            Command::ApplyRouting(rules, _) => rules,
            _ => panic!("expected RU rule update"),
        };
        assert!(rules.contains(&"RULE-SET,RU-SITES,MULTILINK".into()));
        assert!(rules.contains(&"DOMAIN-SUFFIX,ru,DIRECT".into()));
    }
    #[test]
    fn connected_rule_and_ru_preset_keep_selected_route_and_commit_after_acknowledgement() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (updates, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        app.settings.nodes = multilink_core::parse_subscription(
            "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#DE-01",
        )
        .unwrap();
        app.connected = true;
        app.action(Action::AddRule);
        app.edit.set_text("https://ozon.com/page");
        app.rule_target = app
            .rule_targets()
            .iter()
            .position(|(id, _)| id == "COUNTRY-DE")
            .unwrap();
        app.action(Action::Submit);
        assert!(
            app.busy && app.settings.rules.is_empty(),
            "do not save before the core acknowledges"
        );
        let (rules, policy) = match received.recv().unwrap() {
            Command::ApplyRouting(rules, policy) => (rules, policy),
            _ => panic!("expected live route update"),
        };
        assert_eq!(rules, ["DOMAIN-SUFFIX,ozon.com,COUNTRY-DE"]);
        updates
            .send(Update::RoutingApplied(
                rules.clone(),
                policy,
                multilink_mihomo::RoutingChange::default(),
            ))
            .unwrap();
        app.drain();
        assert_eq!(app.settings.rules, rules);
        assert!(matches!(received.recv().unwrap(), Command::Save(_)));
        app.action(Action::AddRuDomains);
        app.rule_target = 1; // general VPN pool
        app.action(Action::Submit);
        let rules = match received.recv().unwrap() {
            Command::ApplyRouting(rules, _) => rules,
            _ => panic!("expected RU route update"),
        };
        assert!(rules.contains(&"RULE-SET,RU-SITES,MULTILINK".into()));
        assert!(rules.contains(&"DOMAIN-SUFFIX,ru,MULTILINK".into()));
        assert!(rules.contains(&"DOMAIN-SUFFIX,xn--p1ai,MULTILINK".into()));
        assert_eq!(rules[0], "DOMAIN-SUFFIX,ozon.com,COUNTRY-DE");
    }
    #[test]
    fn test_connected_server_toggling_and_disabled_hints() {
        let mut app = App::new(None);
        let (commands, received) = mpsc::channel();
        app.tx = commands;
        let (_, receiver) = mpsc::channel();
        app.rx = receiver;
        app.settings = Settings::default();
        let mut n1 = multilink_core::parse_subscription("vless://00000000-0000-4000-8000-000000000001@example.invalid:443#DE-01").unwrap().pop().unwrap();
        n1.name = "Server 1".into();
        n1.enabled = true;
        let mut n2 = n1.clone();
        n2.name = "Server 2".into();
        n2.enabled = true;
        app.settings.nodes = vec![n1, n2];
        app.connected = true;

        // Toggle server 1 while connected
        app.action(Action::Toggle(0));
        assert!(!app.settings.nodes[0].enabled);
        assert!(matches!(received.try_recv(), Ok(Command::Save(_))));
        assert!(matches!(received.try_recv(), Ok(Command::ApplySettings(_))));

        // Attempting to toggle off the last remaining server should be refused
        app.action(Action::Toggle(1));
        assert!(app.settings.nodes[1].enabled);
        assert!(app.notice.contains("Нельзя отключить единственный активный сервер"));

        // Check disabled hints on Settings page
        app.compact = false;
        app.page = Page::Settings;
        app.paint(1040.0, 740.0);
        assert!(app.disabled_hits.iter().any(|(_, hint)| hint.contains("Отключите VPN")));
    }
    #[test]
    fn pin_lock_protection_and_unlock_flow() {
        let mut app = App::new(None);
        let (commands, _received) = mpsc::channel();
        app.tx = commands;
        app.settings = Settings::default();
        app.compact = false;

        // Set PIN
        app.action(Action::OpenSetPin);
        assert_eq!(app.modal, Some(Modal::SetPin));
        app.edit.set_text("1234");
        app.action(Action::Submit);
        assert_eq!(app.settings.pin_code.as_deref(), Some("1234"));
        assert!(app.pin_unlocked);

        // Lock PIN
        app.action(Action::LockPin);
        assert!(!app.pin_unlocked);
        assert_eq!(app.page, Page::Dashboard);

        // Navigation to Settings is blocked by PIN
        app.action(Action::Nav(Page::Settings));
        assert_eq!(app.modal, Some(Modal::PinAuth));
        assert_ne!(app.page, Page::Settings);

        // Incorrect PIN
        app.edit.set_text("9999");
        app.action(Action::Submit);
        assert_eq!(app.modal, Some(Modal::PinAuth));
        assert!(app.notice.contains("Неверный PIN-код"));

        // Correct PIN unlocks and proceeds to target page
        app.edit.set_text("1234");
        app.action(Action::Submit);
        assert!(app.pin_unlocked);
        assert_eq!(app.modal, None);
        assert_eq!(app.page, Page::Settings);

        // Remove PIN
        app.action(Action::RemovePin);
        assert_eq!(app.settings.pin_code, None);
        assert!(!app.pin_locked());
    }
    #[test]
    fn autoconnect_and_rules_only_mode_toggles() {
        let mut app = App::new(None);
        let (commands, _received) = mpsc::channel();
        app.tx = commands;
        app.settings = Settings::default();

        assert!(!app.settings.autoconnect);
        app.action(Action::ToggleAutoconnect);
        assert!(app.settings.autoconnect);
        app.action(Action::ToggleAutoconnect);
        assert!(!app.settings.autoconnect);

        assert!(!app.settings.rules_only);
        app.action(Action::ToggleRulesOnly);
        assert!(app.settings.rules_only);
        app.action(Action::ToggleRulesOnly);
        assert!(!app.settings.rules_only);
    }
    #[test]
    fn gaming_mode_rule_targets_available() {
        let app = App::new(None);
        let targets = app.rule_targets();
        assert!(targets.iter().any(|(id, _)| id == "MODE-LATENCY"));
        assert!(targets.iter().any(|(id, _)| id == "MODE-SMART"));
        assert!(targets.iter().any(|(id, _)| id == "MODE-STABLE"));
        assert!(targets.iter().any(|(id, _)| id == "MODE-BALANCE"));
    }
}
