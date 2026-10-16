//! 服务管理器（HLD §3.2）：单写者应用配置变更（RD-4），每服务一个监督任务 + 一个日志 hub。
//! 本文件为类型与装配；启停/查询 API 见 control.rs，配置重载见 reload.rs，日志 hub 见 hub.rs。

pub mod control;
pub mod hub;
pub mod platform;
pub mod reload;
pub mod shellenv;
pub mod supervisor;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::{mpsc, watch};

use crate::config::{AppConfig, GlobalSettings, ServiceConfig};
use crate::events::{CoreEvent, EventBus, MatchItem, StateInfo, StopReason, Stream, SvcState};
use crate::log::LogStore;
use crate::matcher::MatcherState;
use crate::term::TermHandle;

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
    /// PTY master 写端（SRS FR-6.4）：spawn 成功时置位、进程退出时清空；
    /// write_stdin 经此转发键盘/粘贴输入。Pipe 模式恒为 None。
    pub pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
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
        // 后台预热登录 shell PATH 缓存（GUI 宿主 PATH 精简兜底，shellenv 模块）
        shellenv::warm();

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
            ServiceManager {
                rt,
                shared,
                config_path,
                _watcher: Some(watcher),
            }
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

    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    pub fn events(&self) -> tokio::sync::broadcast::Receiver<CoreEvent> {
        self.shared.bus.subscribe()
    }

    fn create_service(shared: &Shared, handle: &tokio::runtime::Handle, cfg: &ServiceConfig) {
        // 监督任务持有「按当前解析链解析后」的配置（DESIGN §4.2）：spawn 用生效值；
        // 原始配置（含全部平台变体）保留在 shared.configs，供存盘与编辑回显。
        // store/matcher 等平台无关字段两种形态取值相同。
        let reg = crate::config::PlatformRegistry::new(&shared.settings.read().unwrap());
        let cfg = cfg.resolve_for(&reg);
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
        let matcher = Arc::new(Mutex::new(MatcherState::new(&cfg.matchers)));
        let term = Arc::new(TermHandle::new(80, 24));
        let (hub_tx, hub_rx) = mpsc::unbounded_channel();
        let got_output = Arc::new(AtomicBool::new(false));

        let ctx = Arc::new(SvcCtx {
            store: store.clone(),
            matcher: matcher.clone(),
            term: term.clone(),
            hub_tx: hub_tx.clone(),
            got_output: got_output.clone(),
            pty_writer: Arc::new(Mutex::new(None)),
        });

        // 日志 hub：批量广播（≤80ms / 64 行），喂 store + matcher
        let hub_id = cfg.id.clone();
        let bus = shared.bus.clone();
        handle.spawn(hub::hub_task(
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
                        frame_bus.send(CoreEvent::TermFrame {
                            id: frame_id.clone(),
                            screen,
                        });
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
            ctx: ctx.clone(),
        };
        handle.spawn(supervisor::supervise(deps, cmd_rx));

        shared.sups.write().unwrap().insert(
            cfg.id.clone(),
            SupHandle {
                cmd_tx,
                cfg_tx,
                state_rx,
            },
        );
        shared.ctxs.write().unwrap().insert(cfg.id.clone(), ctx);
    }
}
