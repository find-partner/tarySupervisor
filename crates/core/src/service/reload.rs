//! ServiceManager 配置重载（reread / update / GUI 保存，CONFIG-SPEC §8）。

use std::collections::HashMap;

use super::{ReloadMode, ServiceManager, SvcCmd};
use crate::config::{
    diff_configs, validate_app, AppConfig, ConfigDelta, PlatformRegistry, ServiceConfig,
};
use crate::events::CoreEvent;

impl ServiceManager {
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
        // diff 按各自解析链比较生效值（DESIGN §4.5）：
        // current_platform / platforms 变更由此反映到生效值比较中
        let delta = diff_configs(
            &old.services,
            &new.services,
            &PlatformRegistry::new(&old.settings),
            &PlatformRegistry::new(&new.settings),
        );
        if mode == ReloadMode::Reread {
            return Ok(delta);
        }
        self.apply_loaded(old, new, delta, true)
    }

    /// GUI 保存路径（SRS FR-2.10 / HLD D-10）：加载并应用新配置——
    /// added/removed/soft 照常应用，changed 的运行中服务**只换配置、不自动重启**，
    /// 重启由用户在"需重启生效"提示中显式触发。
    pub fn apply_saved_config(&self) -> Result<ConfigDelta, Vec<String>> {
        let new = crate::config::load_app_config(&self.config_path).map_err(|e| {
            e.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        })?;
        let errors = validate_app(&new);
        if !errors.is_empty() {
            return Err(errors);
        }
        let old = self.current_config();
        let delta = diff_configs(
            &old.services,
            &new.services,
            &PlatformRegistry::new(&old.settings),
            &PlatformRegistry::new(&new.settings),
        );
        self.apply_loaded(old, new, delta, false)
    }

    fn apply_loaded(
        &self,
        old: AppConfig,
        new: AppConfig,
        delta: ConfigDelta,
        restart_changed: bool,
    ) -> Result<ConfigDelta, Vec<String>> {
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
        // 监督任务收到「按当前解析链解析后」的配置（DESIGN §4.2）：
        // spawn/展示用生效值，原始配置（含全部平台变体）保留在 shared.configs 供存盘/编辑
        let reg = PlatformRegistry::new(&self.shared.settings.read().unwrap());
        let resolved = |cfg: &ServiceConfig| cfg.resolve_for(&reg);
        let old_map: HashMap<String, ServiceConfig> = old
            .services
            .iter()
            .map(|s| (s.id.clone(), s.clone()))
            .collect();
        let new_map: HashMap<String, ServiceConfig> = new
            .services
            .iter()
            .map(|s| (s.id.clone(), s.clone()))
            .collect();

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
                ServiceManager::create_service(&self.shared, handle, cfg);
            }
        }
        // changed：仅 name/autostart 变更即时生效不重启（soft）；
        // restart_changed=false（GUI 保存）时只换配置不重启（SRS FR-2.10）
        for id in &delta.changed {
            let Some(cfg) = new_map.get(id) else { continue };
            let sups = self.shared.sups.read().unwrap();
            if let Some(sup) = sups.get(id) {
                let _ = sup.cfg_tx.send(resolved(cfg));
                // 日志文件策略与 Matcher 即时更新
                if let Some(ctx) = self.shared.ctxs.read().unwrap().get(id) {
                    let _ = ctx.store.replace_file(cfg.log.file.as_ref());
                    if old_map
                        .get(id)
                        .map(|o| o.matchers != cfg.matchers)
                        .unwrap_or(true)
                    {
                        ctx.matcher.lock().unwrap().replace(&cfg.matchers);
                    }
                }
                if restart_changed {
                    let _ = sup.cmd_tx.send(SvcCmd::Restart);
                }
            }
        }
        for id in &delta.soft_changed {
            if let Some(cfg) = new_map.get(id) {
                if let Some(sup) = self.shared.sups.read().unwrap().get(id) {
                    let _ = sup.cfg_tx.send(resolved(cfg));
                }
            }
        }

        self.shared.bus.send(CoreEvent::ConfigChanged {
            delta: delta.clone(),
        });
        Ok(delta)
    }

    /// 从配置文件重新加载（热加载 watcher 与 GUI 保存共用）
    pub fn reload_from_file(&self) -> Result<ConfigDelta, Vec<String>> {
        match crate::config::load_app_config(&self.config_path) {
            Ok(cfg) => self.reload(cfg, ReloadMode::Update),
            Err(e) => Err(e
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()),
        }
    }

    /// 热加载校验失败事件（watcher 调用，FR-2.6 提示）
    pub fn publish_config_invalid(&self, errors: Vec<String>) {
        self.shared.bus.send(CoreEvent::ConfigInvalid { errors });
    }
}
