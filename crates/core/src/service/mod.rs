//! 服务管理器（HLD §3.2）：单写者应用配置变更（RD-4），每服务一个监督任务 + 一个日志 hub。

pub mod platform;
pub mod supervisor;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::{mpsc, watch};

use crate::config::{
    diff_configs, effective_matchers, validate_app, AppConfig, ConfigDelta, GlobalSettings,
    ServiceConfig,
};
use crate::events::{
    CoreEvent, EventBus, LogLine, MatchItem, StateInfo, StopReason, Stream, SvcState,
};
use crate::log::LogStore;
use crate::matcher::MatcherState;
use crate::term::{TermHandle, TermScreen};

/// 监督任务控制命令
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvcCmd {
    Start,
    Stop,
    Restart,
    /// 停止后任务退出（服务被移除 / 应用退出）
    Shutdown,
}

/// 日志 hub 输入：Line 走「首行输出」检测；System 为托管系统消息（不参与 STARTING 判定）
#[derive(Debug)]
pub enum HubMsg {
    Line(Stream, String),
    System(String),
}

/// 每服务共享上下文
pub struct SvcCtx {
    pub store: Arc<LogStore>,
    pub matcher: Arc<Mutex<MatcherState>>,
    pub term: Arc<TermHandle>,
    pub hub_tx: mpsc::UnboundedSender<HubMsg>,
    pub got_output: Arc<AtomicBool>,
}

#[derive(Clone)]
struct SupHandle {
    cmd_tx: mpsc::UnboundedSender<SvcCmd>,
    cfg_tx: watch::Sender<ServiceConfig>,
    state_rx: watch::Receiver<StateInfo>,
}

#[derive(Default)]
struct Shared {
    bus: EventBus,
    settings: RwLock<GlobalSettings>,
    configs: RwLock<Vec<ServiceConfig>>,
    sups: RwLock<HashMap<String, SupHandle>>,
    ctxs: RwLock<HashMap<String, Arc<SvcCtx>>>,
}

/// 服务运行态投影（前端 list_services / 托盘菜单构建的数据源）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceBrief {
    pub config: ServiceConfig,
    pub state: SvcState,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub reason: Option<StopReason>,
    /// 进入当前状态的时间戳（毫秒）
    pub since_ms: u64,
    pub match_items: Vec<MatchItem>,
}

/// 配置重载模式（HLD §4.1 reload_config）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReloadMode {
    /// 仅报告差异，不应用
    Reread,
    /// 应用差异（removed 停止移除 / changed 重启 / added 加入菜单不自动启动）
    Update,
}

/// 核心管理器：持有独立 tokio runtime，全部对外 API 同步非阻塞（NFR-8）。
pub struct ServiceManager {
    rt: tokio::runtime::Runtime,
    shared: Arc<Shared>,
    config_path: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl ServiceManager {
    /// 创建管理器并启动全部服务任务（autostart 服务自动 Start）。
    ///
    /// 必须在非 async 上下文调用（内部创建独立 runtime）。
    pub fn new(config: AppConfig, config_path: PathBuf) -> Result<Arc<Self>, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("traysv-core")
            .enable_all()
            .build()
            .map_err(|e| format!("创建核心 runtime 失败：{e}"))?;

        let shared = Arc::new(Shared {
            bus: EventBus::new(),
            settings: RwLock::new(config.settings.clone()),
            configs: RwLock::new(Vec::new()),
            sups: RwLock::new(HashMap::new()),
            ctxs: RwLock::new(HashMap::new()),
        });

        let mgr = Arc::new_cyclic(|weak: &std::sync::Weak<ServiceManager>| {
            // 配置文件热加载（FR-2.6）
            let watcher = crate::watcher::spawn(config_path.clone(), weak.clone());
            ServiceManager { rt, shared, config_path, _watcher: Some(watcher) }
        });

        // 初始服务
        {
            let handle = mgr.rt.handle();
            for svc in &config.services {
                Self::create_service(&mgr.shared, handle, svc);
            }
        }
        *mgr.shared.configs.write().unwrap() = config.services.clone();

        // autostart（FR-2.1：应用启动时自动启动）
        for svc in &config.services {
            if svc.autostart {
                if let Some(sup) = mgr.shared.sups.read().unwrap().get(&svc.id) {
                    let _ = sup.cmd_tx.send(SvcCmd::Start);
                }
            }
        }

        Ok(mgr)
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn events(&self) -> tokio::sync::broadcast::Receiver<CoreEvent> {
        self.shared.bus.subscribe()
    }

    fn create_service(shared: &Shared, handle: &tokio::runtime::Handle, cfg: &ServiceConfig) {
        let cap = shared.settings.read().unwrap().log_lines as usize;
        let store = match &cfg.log.file {
            Some(f) => match LogStore::new(cap, Some(f)) {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    log::warn!("服务 {} 日志文件不可用：{e}", cfg.id);
                    Arc::new(LogStore::new(cap, None).expect("纯内存 LogStore 不会失败"))
                }
            },
            None => Arc::new(LogStore::new(cap, None).expect("纯内存 LogStore 不会失败")),
        };
        let matcher =
            Arc::new(Mutex::new(MatcherState::new(&effective_matchers(cfg))));
        let term = Arc::new(TermHandle::new(80, 24));
        let (hub_tx, hub_rx) = mpsc::unbounded_channel();
        let got_output = Arc::new(AtomicBool::new(false));

        let ctx = Arc::new(SvcCtx {
            store: store.clone(),
            matcher: matcher.clone(),
            term: term.clone(),
            hub_tx: hub_tx.clone(),
            got_output: got_output.clone(),
        });

        // 日志 hub：批量广播（≤80ms / 64 行），喂 store + matcher
        let hub_id = cfg.id.clone();
        let bus = shared.bus.clone();
        handle.spawn(hub_task(
            hub_id,
            hub_rx,
            store.clone(),
            matcher.clone(),
            bus,
            got_output.clone(),
        ));
        // 终端帧发射：PTY 输出 → 屏幕快照事件，≤15fps 合并（HLD D-3 / RD-1）
        {
            let frame_id = cfg.id.clone();
            let frame_term = term.clone();
            let frame_bus = shared.bus.clone();
            handle.spawn(async move {
                loop {
                    frame_term.wait_output().await;
                    tokio::time::sleep(std::time::Duration::from_millis(66)).await;
                    if frame_term.has_dirty() {
                        let screen = frame_term.take_screen();
                        frame_bus.send(CoreEvent::TermFrame { id: frame_id.clone(), screen });
                    }
                }
            });
        }

        let (cfg_tx, cfg_rx) = watch::channel(cfg.clone());
        let (state_tx, state_rx) = watch::channel(StateInfo {
            state: SvcState::Stopped,
            pid: None,
            exit_code: None,
            reason: None,
            since_ms: 0,
        });
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        let deps = supervisor::SupervisorDeps {
            id: cfg.id.clone(),
            bus: shared.bus.clone(),
            cfg_rx,
            state_tx,
            hub_tx,
            got_output,
            matcher,
            term,
        };
        handle.spawn(supervisor::supervise(deps, cmd_rx));

        shared.sups.write().unwrap().insert(
            cfg.id.clone(),
            SupHandle { cmd_tx, cfg_tx, state_rx },
        );
        shared.ctxs.write().unwrap().insert(cfg.id.clone(), ctx);
    }

    pub fn list(&self) -> Vec<ServiceBrief> {
        let configs = self.shared.configs.read().unwrap().clone();
        configs
            .iter()
            .map(|cfg| {
                let sup = self.shared.sups.read().unwrap().get(&cfg.id).cloned();
                let (state, pid, exit_code, reason, since_ms) = match sup {
                    Some(s) => {
                        let i = s.state_rx.borrow().clone();
                        (i.state, i.pid, i.exit_code, i.reason, i.since_ms)
                    }
                    None => (SvcState::Stopped, None, None, None, 0),
                };
                let match_items = self.match_items(&cfg.id);
                ServiceBrief {
                    config: cfg.clone(),
                    state,
                    pid,
                    exit_code,
                    reason,
                    since_ms,
                    match_items,
                }
            })
            .collect()
    }

    fn send_cmd(&self, id: &str, cmd: SvcCmd) -> Result<(), String> {
        let sups = self.shared.sups.read().unwrap();
        let sup = sups.get(id).ok_or_else(|| format!("服务 {id} 不存在"))?;
        sup.cmd_tx
            .send(cmd)
            .map_err(|_| format!("服务 {id} 的监督任务已退出"))?;
        Ok(())
    }

    fn state_of(&self, id: &str) -> Option<SvcState> {
        let sups = self.shared.sups.read().unwrap();
        sups.get(id).map(|s| s.state_rx.borrow().state)
    }

    pub fn start(&self, id: &str) -> Result<(), String> {
        match self.state_of(id) {
            None => Err(format!("服务 {id} 不存在")),
            Some(st) if matches!(st, SvcState::Stopped | SvcState::Exited | SvcState::Fatal | SvcState::Backoff) => {
                self.send_cmd(id, SvcCmd::Start)
            }
            Some(st) => Err(format!("服务 {id} 当前为 {}，无需启动", st.as_str())),
        }
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        match self.state_of(id) {
            None => Err(format!("服务 {id} 不存在")),
            Some(st) if matches!(st, SvcState::Starting | SvcState::Running | SvcState::Stopping | SvcState::Backoff) => {
                self.send_cmd(id, SvcCmd::Stop)
            }
            Some(st) => Err(format!("服务 {id} 当前为 {}，无需停止", st.as_str())),
        }
    }

    pub fn restart(&self, id: &str) -> Result<(), String> {
        match self.state_of(id) {
            None => Err(format!("服务 {id} 不存在")),
            Some(SvcState::Stopping) => Err(format!("服务 {id} 正在停止，请稍后重试")),
            Some(_) => self.send_cmd(id, SvcCmd::Restart),
        }
    }

    pub fn get_log(&self, id: &str, since: u64, limit: usize) -> Option<Vec<LogLine>> {
        let ctxs = self.shared.ctxs.read().unwrap();
        ctxs.get(id).map(|c| c.store.snapshot(since, limit))
    }

    pub fn match_items(&self, id: &str) -> Vec<MatchItem> {
        let ctxs = self.shared.ctxs.read().unwrap();
        match ctxs.get(id) {
            Some(c) => c.matcher.lock().unwrap().items(),
            None => vec![],
        }
    }

    /// 按哈希查找快捷项 URL（托盘 match:<id>:<hash> 事件路由，FR-1.7 / FR-5.6）
    pub fn match_url(&self, id: &str, hash: u64) -> Option<String> {
        let ctx = self.shared.ctxs.read().unwrap().get(id).cloned();
        let Some(ctx) = ctx else { return None };
        let items = ctx.matcher.lock().unwrap().items();
        items.into_iter().find(|i| i.hash == hash).and_then(|i| i.url)
    }

    /// 只读终端快照（FR-6.2：视图只是输出订阅者，PTY master 由后端持有）
    pub fn term_snapshot(&self, id: &str) -> Option<TermScreen> {
        let ctx = self.shared.ctxs.read().unwrap().get(id).cloned();
        let Some(ctx) = ctx else { return None };
        Some(ctx.term.screen())
    }

    /// 当前生效配置
    pub fn current_config(&self) -> AppConfig {
        AppConfig {
            settings: self.shared.settings.read().unwrap().clone(),
            services: self.shared.configs.read().unwrap().clone(),
        }
    }

    /// reread / update：校验 → diff →（可选）串行应用（CONFIG-SPEC §8）
    pub fn reload(&self, new: AppConfig, mode: ReloadMode) -> Result<ConfigDelta, Vec<String>> {
        let errors = validate_app(&new);
        if !errors.is_empty() {
            return Err(errors);
        }
        let old = self.current_config();
        let delta = diff_configs(&old.services, &new.services);
        if mode == ReloadMode::Reread {
            return Ok(delta);
        }
        if delta.is_empty() {
            return Ok(delta);
        }

        // settings 变更即时生效（环形缓冲容量）
        if old.settings.log_lines != new.settings.log_lines {
            let ctxs = self.shared.ctxs.read().unwrap();
            for ctx in ctxs.values() {
                ctx.store.set_cap(new.settings.log_lines as usize);
            }
        }
        *self.shared.settings.write().unwrap() = new.settings.clone();
        *self.shared.configs.write().unwrap() = new.services.clone();

        let handle = self.rt.handle();
        let old_map: HashMap<String, ServiceConfig> =
            old.services.iter().map(|s| (s.id.clone(), s.clone())).collect();
        let new_map: HashMap<String, ServiceConfig> =
            new.services.iter().map(|s| (s.id.clone(), s.clone())).collect();

        // removed：优雅停止后移出菜单（CONFIG-SPEC §8）
        for id in &delta.removed {
            if let Some(sup) = self.shared.sups.read().unwrap().get(id) {
                let _ = sup.cmd_tx.send(SvcCmd::Shutdown);
            }
            self.shared.sups.write().unwrap().remove(id);
            self.shared.ctxs.write().unwrap().remove(id);
        }
        // added：加入菜单，不自动启动
        for id in &delta.added {
            if let Some(cfg) = new_map.get(id) {
                Self::create_service(&self.shared, handle, cfg);
            }
        }
        // changed：重启使配置生效；仅 name/autostart 变更即时生效不重启（soft）
        for id in &delta.changed {
            let Some(cfg) = new_map.get(id) else { continue };
            let sups = self.shared.sups.read().unwrap();
            if let Some(sup) = sups.get(id) {
                let _ = sup.cfg_tx.send(cfg.clone());
                // 日志文件策略与 Matcher 即时更新
                if let Some(ctx) = self.shared.ctxs.read().unwrap().get(id) {
                    let _ = ctx.store.replace_file(cfg.log.file.as_ref());
                    if old_map.get(id).map(|o| o.matchers != cfg.matchers).unwrap_or(true) {
                        ctx.matcher.lock().unwrap().replace(&effective_matchers(cfg));
                    }
                }
                let _ = sup.cmd_tx.send(SvcCmd::Restart);
            }
        }
        for id in &delta.soft_changed {
            if let Some(cfg) = new_map.get(id) {
                if let Some(sup) = self.shared.sups.read().unwrap().get(id) {
                    let _ = sup.cfg_tx.send(cfg.clone());
                }
            }
        }

        self.shared.bus.send(CoreEvent::ConfigChanged { delta: delta.clone() });
        Ok(delta)
    }

    /// 从配置文件重新加载（热加载 watcher 与 GUI 保存共用）
    pub fn reload_from_file(&self) -> Result<ConfigDelta, Vec<String>> {
        match crate::config::load_app_config(&self.config_path) {
            Ok(cfg) => self.reload(cfg, ReloadMode::Update),
            Err(e) => Err(e.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()),
        }
    }

    /// 热加载校验失败事件（watcher 调用，FR-2.6 提示）
    pub fn publish_config_invalid(&self, errors: Vec<String>) {
        self.shared.bus.send(CoreEvent::ConfigInvalid { errors });
    }

    /// 运行中服务数（托盘 tooltip：运行中 n / 共 m 个服务）
    pub fn running_count(&self) -> usize {
        let sups = self.shared.sups.read().unwrap();
        sups.values().filter(|s| s.state_rx.borrow().state.is_alive()).count()
    }

    pub fn total_count(&self) -> usize {
        self.shared.sups.read().unwrap().len()
    }

    /// 应用退出：对全部运行中服务执行停止流程并等待完成（FR-3.7）。
    /// 阻塞直至全部停止（上限 = 最大 stop_timeout + 10s）。
    pub fn shutdown_all(&self) {
        {
            let sups = self.shared.sups.read().unwrap();
            for sup in sups.values() {
                let _ = sup.cmd_tx.send(SvcCmd::Shutdown);
            }
            let max_timeout = self
                .shared
                .configs
                .read()
                .unwrap()
                .iter()
                .map(|s| s.stop_timeout_secs)
                .max()
                .unwrap_or(10);
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(max_timeout + 10);
            loop {
                let alive = sups
                    .values()
                    .filter(|s| s.state_rx.borrow().state.is_alive())
                    .count();
                if alive == 0 || std::time::Instant::now() >= deadline {
                    if alive > 0 {
                        log::warn!("退出等待超时，仍有 {alive} 个服务未完全停止");
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// 日志 hub：行缓冲批量广播（HLD §3.3）
async fn hub_task(
    id: String,
    mut rx: mpsc::UnboundedReceiver<HubMsg>,
    store: Arc<LogStore>,
    matcher: Arc<Mutex<MatcherState>>,
    bus: EventBus,
    got_output: Arc<AtomicBool>,
) {
    const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);
    let mut batch: Vec<LogLine> = Vec::new();
    let mut matcher_dirty = false;
    let mut last_flush = std::time::Instant::now();

    loop {
        let flush_due = !batch.is_empty() && last_flush.elapsed() >= FLUSH_INTERVAL;
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    None => break,
                    Some(HubMsg::Line(stream, text)) => {
                        got_output.store(true, Ordering::Relaxed);
                        let line = store.push(stream, &text);
                        matcher_dirty |= matcher.lock().unwrap().feed_line(&text);
                        batch.push(line);
                    }
                    Some(HubMsg::System(text)) => {
                        let line = store.push(Stream::Out, &text);
                        batch.push(line);
                    }
                }
            }
            _ = tokio::time::sleep(FLUSH_INTERVAL), if flush_due => {}
        }
        if !batch.is_empty() && (batch.len() >= 64 || last_flush.elapsed() >= FLUSH_INTERVAL) {
            bus.send(CoreEvent::LogAppended { id: id.clone(), lines: std::mem::take(&mut batch) });
            if matcher_dirty {
                matcher_dirty = false;
                let items = matcher.lock().unwrap().items();
                bus.send(CoreEvent::MatcherHit { svc_id: id.clone(), items });
            }
            last_flush = std::time::Instant::now();
        }
    }
    if !batch.is_empty() {
        bus.send(CoreEvent::LogAppended { id, lines: batch });
    }
}
