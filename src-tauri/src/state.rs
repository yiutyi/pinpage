// 功能：维护应用状态，合并窗口变化并串行持久化配置。
use crate::{
    errors::AppMessage,
    i18n,
    import::ImportGrant,
    models::{AppLocale, PinRuntime, Settings, Snapshot},
    platform::Geometry,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

/// 工具条自动显隐的宽限期：光标进入热区或悬停后继续显示这么久。
pub const TOOLBAR_GRACE: Duration = Duration::from_secs(2);

/// 工具条的用户意图与临时门闩，三个字段各有唯一写入方，彼此不覆盖。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolbarState {
    pub pinned: bool,
    pub dismissed: bool,
    pub hold: bool,
}

/// 工具条运行时状态，按磁贴（标签组）ID 存一份，不写入会话配置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolbarRuntime {
    pub state: ToolbarState,
    pub grace_until: Option<Instant>,
    pub visible: bool,
}

impl ToolbarRuntime {
    /// 只替换用户意图，保留宽限期与上一帧显隐。
    pub fn with_state(self, next: ToolbarState) -> Self {
        Self {
            state: next,
            ..self
        }
    }

    /// 页面关闭时只清临时状态，用户的图钉偏好保留到下次开启。
    pub fn reset_transient(self) -> Self {
        Self {
            state: ToolbarState {
                pinned: self.state.pinned,
                ..Default::default()
            },
            grace_until: None,
            visible: false,
        }
    }
}

/// 标签组里的一个标签：窗口标签用于原生操作，标题用于工具条显示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabEntry {
    pub window: String,
    pub title: String,
}

/// 一个磁贴的标签组；首页窗口始终排在第一位，只存在于本次运行。
#[derive(Clone, Debug)]
pub struct TabGroup {
    pub tabs: Vec<TabEntry>,
    pub active: String,
    pub next: usize,
    pub geometry: Option<Geometry>,
}

/// 前台唤出状态：只记录被临时抬起的页面与唤出前的前台窗口。
#[derive(Default)]
pub struct LiftState {
    pub previous: isize,
    pub pages: HashMap<String, String>,
}

pub struct AppState {
    pub operation: Mutex<()>,
    pub settings: Mutex<Settings>,
    pub runtime: Mutex<HashMap<String, PinRuntime>>,
    /// 每个标签组一份工具条运行时状态，不写入会话配置。
    pub toolbars: Mutex<HashMap<String, ToolbarRuntime>>,
    /// 各磁贴的标签组，只在本次运行有效。
    pub tabs: Mutex<HashMap<String, TabGroup>>,
    /// 前台唤出状态，只在本次运行有效。
    pub lift: Mutex<LiftState>,
    /// WebView2 外观脚本的注册 ID；关闭窗口后清理，不写入配置。
    #[cfg(windows)]
    pub appearances: Mutex<HashMap<String, (bool, crate::models::WindowMode, String)>>,
    /// 原生选择器签发的一次性导入授权，创建成功后不再复用。
    pub grants: Mutex<HashMap<String, ImportGrant>>,
    /// 正在导出的页面 ID，只有这些页面可以回传数据。
    pub export_pending: Mutex<HashSet<String>>,
    /// 窗口菜单签发的「页面设置」直达意图；管理窗口取走后立即清空，不写入配置。
    pub pending_pin_settings: Mutex<Option<String>>,
    /// 已回传的页面数据，导出命令取走后立即清空。
    pub export_data: Mutex<HashMap<String, String>>,
    /// 共享系统采样器的最新快照；内容页监控命令只读缓存，不触发采样。
    pub stats: Mutex<Option<crate::host::SystemStats>>,
    pub notices: Mutex<Vec<AppMessage>>,
    /// Rust 解析后的有效语言，是原生界面与快照的唯一来源。
    pub locale: Mutex<AppLocale>,
    pub exiting: AtomicBool,
    /// 安装期间冻结配置；失败后可恢复，退出标记只在 installer 真正启动前设置。
    pub installing: AtomicBool,
    pub read_only: bool,
    /// 本次加载是否发生过平台能力回退，用于启动后持久化降级结果。
    pub platform_fallback: bool,
    revision: AtomicU64,
    saving: AtomicBool,
    writer: Mutex<()>,
    path: PathBuf,
}

impl AppState {
    /// 加载配置；未来版本只读保护，损坏文件保留备份，不覆盖未知结构。
    pub fn load(path: PathBuf) -> Self {
        let mut notices = Vec::new();
        let mut read_only = false;
        let mut settings = match std::fs::read(&path) {
            Ok(bytes)
                if serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|value| value.get("schemaVersion").and_then(|value| value.as_u64()))
                    .is_some_and(|version| version > crate::models::SCHEMA_VERSION as u64) =>
            {
                read_only = true;
                notices.push(AppMessage::new("errors.configNewer"));
                Settings::default()
            }
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(value)
                    if value.schema_version == crate::models::SCHEMA_VERSION
                        || value.schema_version == 1 =>
                {
                    value
                }
                _ => {
                    let backup =
                        path.with_extension(format!("backup-{}.json", uuid::Uuid::new_v4()));
                    match std::fs::rename(&path, &backup) {
                        Ok(_) => notices.push(
                            AppMessage::new("errors.configBackedUp")
                                .arg("path", backup.display().to_string()),
                        ),
                        Err(error) => {
                            notices.push(AppMessage::new("errors.configBackupFailed").detail(error))
                        }
                    }
                    Settings::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(error) => {
                notices.push(AppMessage::new("errors.readConfigFailed").detail(error));
                Settings::default()
            }
        };
        // v1 → v2：预设从全局库迁移到页面。只把被页面引用的模板复制进该页自己的
        // 列表，未被引用的模板丢弃；外观与几何不发生任何变化。
        if settings.schema_version == 1 {
            for pin in &mut settings.pins {
                if let Some(id) = pin.preset.clone() {
                    if let Some(preset) = settings
                        .window_presets
                        .iter()
                        .find(|preset| preset.id == id)
                        .cloned()
                    {
                        pin.presets.push(preset);
                    }
                }
            }
            settings.schema_version = crate::models::SCHEMA_VERSION;
        }
        settings.window_presets.clear();
        // 配置可能来自 Windows：加载时先降级到当前平台可用的组合，避免恢复会话反复失败。
        let mut fallback = false;
        for pin in &mut settings.pins {
            fallback |= crate::platform::fallback_pin(crate::platform::capabilities(), pin);
        }
        if fallback {
            notices.push(AppMessage::new("errors.platformFallback"));
        }
        // 指向页内不存在模板的标记收敛为“自定义”，窗口外观保持不变。
        for pin in &mut settings.pins {
            if pin
                .preset
                .as_deref()
                .is_some_and(|id| !pin.presets.iter().any(|preset| preset.id == id))
            {
                pin.preset = None;
            }
        }
        let locale = i18n::resolve(settings.locale);
        Self {
            operation: Mutex::new(()),
            #[cfg(windows)]
            appearances: Mutex::new(HashMap::new()),
            settings: Mutex::new(settings),
            runtime: Mutex::new(HashMap::new()),
            toolbars: Mutex::new(HashMap::new()),
            tabs: Mutex::new(HashMap::new()),
            lift: Mutex::new(LiftState::default()),
            grants: Mutex::new(HashMap::new()),
            export_pending: Mutex::new(HashSet::new()),
            pending_pin_settings: Mutex::new(None),
            export_data: Mutex::new(HashMap::new()),
            stats: Mutex::new(None),
            notices: Mutex::new(notices),
            locale: Mutex::new(locale),
            exiting: AtomicBool::new(false),
            installing: AtomicBool::new(false),
            read_only,
            platform_fallback: fallback,
            revision: AtomicU64::new(0),
            saving: AtomicBool::new(false),
            writer: Mutex::new(()),
            path,
        }
    }

    /// 汇总配置及短期运行态，管理界面重建后可重新读取。
    pub fn snapshot(&self) -> Snapshot {
        let settings = self.settings.lock().unwrap().clone();
        Snapshot {
            settings,
            runtime: self.runtime.lock().unwrap().clone(),
            notices: self.notices.lock().unwrap().clone(),
            resolved_locale: *self.locale.lock().unwrap(),
            capabilities: crate::platform::capabilities(),
        }
    }

    /// 在同目录写入临时文件后原子替换，避免半写入的配置被加载。
    pub fn save(&self) -> Result<(), AppMessage> {
        let _writer = self.writer.lock().unwrap();
        self.ensure_mutable()?;
        self.write_config()
    }

    /// 所有配置修改入口与安装共用检查，未知配置版本和安装冻结都不能写入。
    pub fn ensure_mutable(&self) -> Result<(), AppMessage> {
        if self.read_only {
            return Err(AppMessage::new("errors.configNewer"));
        }
        if self.installing.load(Ordering::SeqCst) {
            return Err(AppMessage::new("errors.updateBusy"));
        }
        Ok(())
    }

    /// 持有操作锁调用；冻结后保存最后快照并备份，任一步失败都中止安装。
    pub fn prepare_update(&self) -> Result<(), AppMessage> {
        let _writer = self.writer.lock().unwrap();
        self.ensure_mutable()?;
        self.installing.store(true, Ordering::SeqCst);
        let result = self.write_config().and_then(|()| {
            let backup = self.path.with_extension("pre-update.json");
            let temporary = self.path.with_extension("pre-update.tmp");
            std::fs::copy(&self.path, &temporary)
                .and_then(|_| replace_file(&temporary, &backup))
                .map_err(|error| AppMessage::new("errors.updateBackupFailed").detail(error))
        });
        if result.is_err() {
            self.installing.store(false, Ordering::SeqCst);
        }
        result
    }

    /// 调用者持有写入锁；原子替换与安装前最终保存共用同一实现。
    fn write_config(&self) -> Result<(), AppMessage> {
        let config = self.settings.lock().unwrap().clone();
        let parent = self
            .path
            .parent()
            .ok_or_else(|| AppMessage::new("errors.configDirInvalid"))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| AppMessage::new("errors.createDirFailed").detail(error))?;
        let bytes = serde_json::to_vec_pretty(&config)
            .map_err(|error| AppMessage::new("errors.saveConfigFailed").detail(error))?;
        let temporary = self.path.with_extension("tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| AppMessage::new("errors.saveConfigFailed").detail(error))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| AppMessage::new("errors.saveConfigFailed").detail(error))?;
        }
        replace_file(&temporary, &self.path)
            .map_err(|error| AppMessage::new("errors.saveConfigFailed").detail(error))
    }
}

/// Windows 的 rename 不覆盖已有文件，使用系统原子替换接口。
#[cfg(windows)]
pub(crate) fn replace_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Unix 的同文件系统 rename 提供原子替换。
#[cfg(not(windows))]
pub(crate) fn replace_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

/// 通知管理界面刷新，不向内容 WebView 广播配置或本地路径。
pub fn notify(app: &AppHandle) {
    let _ = app.emit_to("main", "pins-changed", ());
}

/// 合并 500ms 内连续的窗口事件，只有最后一个修订触发落盘。
pub fn schedule_save(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.installing.load(Ordering::SeqCst) || state.read_only {
        return;
    }
    state.revision.fetch_add(1, Ordering::SeqCst);
    if state.saving.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        loop {
            let revision = state.revision.load(Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(500));
            if state.revision.load(Ordering::SeqCst) != revision {
                continue;
            }
            if !state.installing.load(Ordering::SeqCst) {
                let _operation = state.operation.lock().unwrap();
                if let Err(error) = state.save() {
                    report(&app, error);
                }
            }
            state.saving.store(false, Ordering::SeqCst);
            // 检查落盘与释放标记之间到来的事件，避免最后一个修订丢失。
            if state.revision.load(Ordering::SeqCst) != revision {
                schedule_save(&app);
            }
            break;
        }
    });
}

/// 记录可见错误，并通知管理界面；限制通知数量避免长期增长。
pub fn report(app: &AppHandle, message: AppMessage) {
    eprintln!("{}", i18n::log_text(&message));
    let state = app.state::<AppState>();
    let mut notices = state.notices.lock().unwrap();
    // 去重只看消息码与参数，诊断详情不同不应产生成排的同类通知。
    let duplicated = notices
        .last()
        .is_some_and(|last| last.code == message.code && last.args == message.args);
    if !duplicated {
        notices.push(message);
    }
    if notices.len() > 10 {
        notices.remove(0);
    }
    drop(notices);
    notify(app);
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "state_update_tests.rs"]
mod update_tests;

/// 读取当前有效语言，供托盘、窗口标题等原生界面渲染。
pub fn locale(app: &AppHandle) -> AppLocale {
    *app.state::<AppState>().locale.lock().unwrap()
}

/// 写入新的有效语言；调用方负责随后的原生界面刷新。
pub fn set_locale(app: &AppHandle, next: AppLocale) {
    *app.state::<AppState>().locale.lock().unwrap() = next;
}

/// 读取标签组的工具条意图；从未操作过的组使用未固定、未收起的默认值。
pub fn toolbar_state(app: &AppHandle, group: &str) -> ToolbarState {
    app.state::<AppState>()
        .toolbars
        .lock()
        .unwrap()
        .get(group)
        .map(|runtime| runtime.state)
        .unwrap_or_default()
}

/// 只改写意图字段；宽限期与显隐由轮询独立维护。
pub fn set_toolbar_state(app: &AppHandle, group: &str, next: ToolbarState) {
    let state = app.state::<AppState>();
    let mut toolbars = state.toolbars.lock().unwrap();
    let runtime = toolbars.entry(group.into()).or_default();
    *runtime = runtime.with_state(next);
}

/// 读取整份运行时状态，供显隐轮询决策。
pub fn toolbar_runtime(app: &AppHandle, group: &str) -> ToolbarRuntime {
    app.state::<AppState>()
        .toolbars
        .lock()
        .unwrap()
        .get(group)
        .copied()
        .unwrap_or_default()
}

/// 写回整份运行时状态；调用方持有同一组的最新快照。
pub fn set_toolbar_runtime(app: &AppHandle, group: &str, next: ToolbarRuntime) {
    app.state::<AppState>()
        .toolbars
        .lock()
        .unwrap()
        .insert(group.into(), next);
}

/// 刷新自动显示宽限期：建窗、热区唤出与悬停共用。
pub fn touch_toolbar_grace(app: &AppHandle, group: &str) {
    let state = app.state::<AppState>();
    let mut toolbars = state.toolbars.lock().unwrap();
    toolbars.entry(group.into()).or_default().grace_until = Some(Instant::now() + TOOLBAR_GRACE);
}

/// 结束自动显示宽限期：主动收起或收回桌面后立即回到常驻策略。
pub fn clear_toolbar_grace(app: &AppHandle, group: &str) {
    if let Some(runtime) = app
        .state::<AppState>()
        .toolbars
        .lock()
        .unwrap()
        .get_mut(group)
    {
        runtime.grace_until = None;
    }
}

/// 页面关闭时只重置临时门闩与倒计时，保留用户的图钉偏好。
pub fn reset_toolbar_runtime(app: &AppHandle, group: &str) {
    if let Some(runtime) = app
        .state::<AppState>()
        .toolbars
        .lock()
        .unwrap()
        .get_mut(group)
    {
        let current = *runtime;
        *runtime = current.reset_transient();
    }
}

/// 删除磁贴时彻底清理工具条状态。
pub fn clear_toolbar_state(app: &AppHandle, group: &str) {
    app.state::<AppState>()
        .toolbars
        .lock()
        .unwrap()
        .remove(group);
}
