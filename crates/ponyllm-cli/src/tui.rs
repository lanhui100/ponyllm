use std::io;
use std::time::Duration;
use crossterm::{
    event::{Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap,
    },
    Frame, Terminal,
};
use serde_json::Value;
use tokio::sync::mpsc;
use crate::config::{ConfigFile, KeySection, ModelConfig};
use ponyllm_core::pool::BillingMode;

pub struct TerminalGuard {
    pub terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen, crossterm::cursor::Show);
            default_hook(panic_info);
        }));

        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab2Focus {
    Providers,
    Models,
}

pub const MODALITIES: [&str; 4] = ["文 (Txt)", "图 (Img)", "视 (Vid)", "音 (Aud)"];
pub const MODALITY_KEYS: [&str; 4] = ["text", "image", "video", "audio"];

pub fn modality_key_to_short(k: &str) -> &'static str {
    match k {
        "text" => "文(Txt)",
        "image" => "图(Img)",
        "video" => "视(Vid)",
        "audio" => "音(Aud)",
        _ => "其",
    }
}
pub const STRATEGIES: [&str; 3] = ["round_robin", "priority", "weighted"];
pub const PROVIDER_BILLING_MODES: [&str; 3] = ["按量付费 (Metered)", "包月订阅 (Coding Plan)", "完全免费 (Free)"];
pub const MODEL_BILLING_MODES: [&str; 4] = ["继承提供商", "按量计费 (Metered)", "包月套餐/Coding Plan (Plan)", "完全免费 (Free)"];

#[derive(Debug, Clone)]
pub enum Modal {
    None,
    AddProvider {
        name: String,
        base_url: String,
        default_model: String,
        strategy_idx: usize,
        billing_mode_idx: usize, // 0: Metered, 1: Plan (Coding Plan), 2: Free
        input_price: String,
        cached_price: String,
        output_price: String,
        active_field: usize, // 0: name, 1: base_url, 2: default_model, 3: strategy, 4: billing_mode, 5: input_price, 6: cached_price, 7: output_price
    },
    EditProvider {
        name: String, // readonly
        base_url: String,
        default_model: String,
        strategy_idx: usize,
        billing_mode_idx: usize,
        input_price: String,
        cached_price: String,
        output_price: String,
        active_field: usize, // 0: base_url, 1: default_model, 2: strategy, 3: billing_mode, 4: input_price, 5: cached_price, 6: output_price
    },
    DeleteProviderConfirm {
        name: String,
    },
    AddModel {
        provider_name: String,
        model_name: String,
        tier_idx: usize, // 0: Flagship, 1: Standard, 2: Light
        billing_mode_idx: usize, // 0: 继承提供商, 1: Metered, 2: Plan (Coding Plan), 3: Free
        input_price: String,
        cached_price: String,
        output_price: String,
        context_window: String,
        max_output: String,
        input_modalities: [bool; 4],
        output_modalities: [bool; 4],
        set_as_default: bool,
        active_field: usize, // 0: name, 1: tier, 2: billing_mode, 3: input_price, 4: cached_price, 5: output_price, 6: context, 7: max_output, 8: inputs, 9: outputs, 10: default
    },
    EditModel {
        provider_name: String,
        model_name: String, // readonly
        tier_idx: usize,
        billing_mode_idx: usize, // 0: 继承提供商, 1: Metered, 2: Plan (Coding Plan), 3: Free
        input_price: String,
        cached_price: String,
        output_price: String,
        context_window: String,
        max_output: String,
        input_modalities: [bool; 4],
        output_modalities: [bool; 4],
        set_as_default: bool,
        active_field: usize, // 0: tier, 1: billing_mode, 2: input_price, 3: cached_price, 4: output_price, 5: context, 6: max_output, 7: inputs, 8: outputs, 9: default
    },
    DeleteModelConfirm {
        provider_name: String,
        model_name: String,
    },
    AddKey {
        provider_idx: usize,   // index into sorted_provider_names()
        id: String,
        api_key: String,
        priority: String,      // editable; parsed to u32 on submit
        weight: String,        // editable; parsed to u32 on submit
        active_field: usize,   // 0: provider, 1: id, 2: api_key, 3: priority, 4: weight
    },
    DeleteKeyConfirm {
        provider: String,
        id: String,
    },
}

pub const TIERS: [ponyllm_core::pool::ModelTier; 3] = [
    ponyllm_core::pool::ModelTier::Flagship,
    ponyllm_core::pool::ModelTier::Standard,
    ponyllm_core::pool::ModelTier::Light,
];

pub fn tier_to_idx(tier: ponyllm_core::pool::ModelTier) -> usize {
    match tier {
        ponyllm_core::pool::ModelTier::Flagship => 0,
        ponyllm_core::pool::ModelTier::Standard => 1,
        ponyllm_core::pool::ModelTier::Light => 2,
    }
}

#[derive(Debug, Clone)]
pub struct TelemetrySnapshot {
    pub is_online: bool,
    pub metrics: Option<Value>,
    pub stream: Option<Value>,
    pub flight_frames: Vec<Value>,
}

pub struct TuiApp {
    pub active_tab: usize,
    pub tab2_focus: Tab2Focus,
    pub config: ConfigFile,
    pub config_path: String,
    pub gateway_url: String,
    pub is_online: bool,
    pub metrics: Option<Value>,
    pub stream: Option<Value>,
    pub flight_frames: Vec<Value>,
    pub provider_table_state: TableState,
    pub model_table_state: TableState,
    pub key_table_state: TableState,
    pub log_table_state: TableState,
    pub modal: Modal,
    pub should_quit: bool,
    pub status_message: String,
}

impl TuiApp {
    pub fn new(config: ConfigFile, config_path: String, gateway_url: String) -> Self {
        let mut provider_state = TableState::default();
        provider_state.select(Some(0));
        let mut model_state = TableState::default();
        model_state.select(Some(0));
        let mut key_state = TableState::default();
        key_state.select(Some(0));
        let mut log_state = TableState::default();
        log_state.select(Some(0));

        Self {
            active_tab: 0,
            tab2_focus: Tab2Focus::Providers,
            config,
            config_path,
            gateway_url,
            is_online: false,
            metrics: None,
            stream: None,
            flight_frames: Vec::new(),
            provider_table_state: provider_state,
            model_table_state: model_state,
            key_table_state: key_state,
            log_table_state: log_state,
            modal: Modal::None,
            should_quit: false,
            status_message: "就绪。按 [Tab] 切换面板，[a] 添加，[e] 编辑，[d] 删除，[q] 退出。".to_string(),
        }
    }

    pub fn sorted_provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.config.providers.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn selected_provider_name(&self) -> Option<String> {
        let names = self.sorted_provider_names();
        if names.is_empty() {
            return None;
        }
        let idx = self.provider_table_state.selected().unwrap_or(0);
        names.get(idx).cloned()
    }

    pub fn selected_models_for_current_provider(&self) -> Vec<ModelConfig> {
        if let Some(p_name) = self.selected_provider_name() {
            if let Some(p) = self.config.providers.get(&p_name) {
                return p.list_all_models();
            }
        }
        Vec::new()
    }

    pub fn selected_model_config(&self) -> Option<ModelConfig> {
        let models = self.selected_models_for_current_provider();
        if models.is_empty() {
            return None;
        }
        let idx = self.model_table_state.selected().unwrap_or(0);
        models.get(idx).cloned()
    }

    /// Resolve the currently selected row in the Key 治理 tab back to its
    /// owning (provider, KeySection) pair, so the row can be deleted.
    pub fn selected_key(&self) -> Option<(String, KeySection)> {
        let mut flat: Vec<(String, KeySection)> = Vec::new();
        for p_name in self.sorted_provider_names() {
            if let Some(p) = self.config.providers.get(&p_name) {
                for k in &p.keys {
                    flat.push((p_name.clone(), k.clone()));
                }
            }
        }
        let idx = self.key_table_state.selected().unwrap_or(0);
        flat.get(idx).cloned()
    }

    pub fn apply_telemetry(&mut self, snap: TelemetrySnapshot) {
        self.is_online = snap.is_online;
        self.metrics = snap.metrics;
        self.stream = snap.stream;
        self.flight_frames = snap.flight_frames;
    }

    pub fn save_config(&mut self) -> Result<(), String> {
        self.config.save_to_path(&self.config_path)
            .map_err(|e| format!("写入配置文件失败: {}", e))
    }
}

async fn fetch_telemetry_snapshot(gateway_url: &str) -> TelemetrySnapshot {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .unwrap_or_default();

    let base = gateway_url.trim_end_matches('/');
    let health_url = format!("{}/health", base);
    let metrics_url = format!("{}/v1/telemetry/metrics", base);
    let stream_url = format!("{}/v1/telemetry/stream", base);
    let rec_url = format!("{}/v1/telemetry/recorder", base);

    if let Ok(resp) = client.get(&health_url).send().await {
        if resp.status().is_success() {
            let mut metrics = None;
            let mut stream = None;
            let mut flight_frames = Vec::new();

            if let Ok(m) = client.get(&metrics_url).send().await {
                if let Ok(val) = m.json::<Value>().await {
                    metrics = Some(val);
                }
            }
            if let Ok(s) = client.get(&stream_url).send().await {
                if let Ok(val) = s.json::<Value>().await {
                    stream = Some(val);
                }
            }
            if let Ok(r) = client.get(&rec_url).send().await {
                if let Ok(val) = r.json::<Value>().await {
                    if let Some(arr) = val.as_array() {
                        flight_frames = arr.clone();
                    }
                }
            }

            return TelemetrySnapshot {
                is_online: true,
                metrics,
                stream,
                flight_frames,
            };
        }
    }

    TelemetrySnapshot {
        is_online: false,
        metrics: None,
        stream: None,
        flight_frames: Vec::new(),
    }
}

pub async fn run_tui(config: ConfigFile, config_path: String, gateway_url: String) -> Result<(), Box<dyn std::error::Error>> {
    let mut guard = TerminalGuard::new()?;
    let mut app = TuiApp::new(config, config_path, gateway_url.clone());

    // 1. 初始化独立异步遥测通道（主循环 0 阻塞）
    let (telemetry_tx, mut telemetry_rx) = mpsc::channel::<TelemetrySnapshot>(10);
    let gw_url_clone = gateway_url.clone();

    tokio::spawn(async move {
        // 先立即拉取一次
        let snap = fetch_telemetry_snapshot(&gw_url_clone).await;
        let _ = telemetry_tx.send(snap).await;

        let mut interval = tokio::time::interval(Duration::from_millis(1500));
        loop {
            interval.tick().await;
            let snap = fetch_telemetry_snapshot(&gw_url_clone).await;
            if telemetry_tx.send(snap).await.is_err() {
                break;
            }
        }
    });

    let mut reader = crossterm::event::EventStream::new();
    let mut render_tick = tokio::time::interval(Duration::from_millis(50));

    loop {
        guard.terminal.draw(|f| ui(f, &mut app))?;

        tokio::select! {
            _ = render_tick.tick() => {
                // 定期消费遥测更新
                while let Ok(snap) = telemetry_rx.try_recv() {
                    app.apply_telemetry(snap);
                }
            }
            maybe_event = reader.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind == KeyEventKind::Press {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                            app.should_quit = true;
                        } else {
                            handle_key_event(&mut app, key.code, key.modifiers);
                        }
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_key_event(app: &mut TuiApp, key: KeyCode, modifiers: KeyModifiers) {
    if !matches!(app.modal, Modal::None) {
        handle_modal_key(app, key, modifiers);
        return;
    }

    match key {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Tab => {
            if app.active_tab == 1 {
                app.tab2_focus = match app.tab2_focus {
                    Tab2Focus::Providers => Tab2Focus::Models,
                    Tab2Focus::Models => Tab2Focus::Providers,
                };
            } else {
                app.active_tab = (app.active_tab + 1) % 4;
            }
        }
        KeyCode::BackTab => {
            if app.active_tab == 1 {
                app.tab2_focus = match app.tab2_focus {
                    Tab2Focus::Providers => Tab2Focus::Models,
                    Tab2Focus::Models => Tab2Focus::Providers,
                };
            } else {
                app.active_tab = if app.active_tab == 0 { 3 } else { app.active_tab - 1 };
            }
        }
        KeyCode::Char('1') => app.active_tab = 0,
        KeyCode::Char('2') => app.active_tab = 1,
        KeyCode::Char('3') => app.active_tab = 2,
        KeyCode::Char('4') => app.active_tab = 3,
        KeyCode::Char('r') => {
            app.status_message = "正在后台刷新遥测数据...".to_string();
        }
        KeyCode::Left | KeyCode::Char('h') if app.active_tab == 1 => {
            app.tab2_focus = Tab2Focus::Providers;
        }
        KeyCode::Right | KeyCode::Char('l') if app.active_tab == 1 => {
            app.tab2_focus = Tab2Focus::Models;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            match app.active_tab {
                0 => {}
                1 => {
                    match app.tab2_focus {
                        Tab2Focus::Providers => {
                            let len = app.config.providers.len();
                            scroll_table(&mut app.provider_table_state, len, 1);
                            let m_len = app.selected_models_for_current_provider().len();
                            scroll_table(&mut app.model_table_state, m_len, 0);
                        }
                        Tab2Focus::Models => {
                            let len = app.selected_models_for_current_provider().len();
                            scroll_table(&mut app.model_table_state, len, 1);
                        }
                    }
                }
                2 => {
                    let total_keys = app.config.providers.values().map(|p| p.keys.len()).sum();
                    scroll_table(&mut app.key_table_state, total_keys, 1);
                }
                3 => scroll_table(&mut app.log_table_state, app.flight_frames.len(), 1),
                _ => {}
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            match app.active_tab {
                0 => {}
                1 => {
                    match app.tab2_focus {
                        Tab2Focus::Providers => {
                            let len = app.config.providers.len();
                            scroll_table(&mut app.provider_table_state, len, -1);
                            let m_len = app.selected_models_for_current_provider().len();
                            scroll_table(&mut app.model_table_state, m_len, 0);
                        }
                        Tab2Focus::Models => {
                            let len = app.selected_models_for_current_provider().len();
                            scroll_table(&mut app.model_table_state, len, -1);
                        }
                    }
                }
                2 => {
                    let total_keys = app.config.providers.values().map(|p| p.keys.len()).sum();
                    scroll_table(&mut app.key_table_state, total_keys, -1);
                }
                3 => scroll_table(&mut app.log_table_state, app.flight_frames.len(), -1),
                _ => {}
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            if app.active_tab == 1 {
                match app.tab2_focus {
                    Tab2Focus::Providers => {
                        app.modal = Modal::AddProvider {
                            name: String::new(),
                            base_url: "https://api.openai.com".to_string(),
                            default_model: "gpt-4o".to_string(),
                            strategy_idx: 0,
                            billing_mode_idx: 0, // 按量计费
                            input_price: "0.50".to_string(),
                            cached_price: "0.25".to_string(),
                            output_price: "1.00".to_string(),
                            active_field: 0,
                        };
                    }
                    Tab2Focus::Models => {
                        if let Some(p_name) = app.selected_provider_name() {
                            app.modal = Modal::AddModel {
                                provider_name: p_name,
                                model_name: String::new(),
                                tier_idx: 1, // Standard
                                billing_mode_idx: 0, // 继承提供商
                                input_price: String::new(),
                                cached_price: String::new(),
                                output_price: String::new(),
                                context_window: "128K".to_string(),
                                max_output: "4K".to_string(),
                                input_modalities: [true, false, false, false],
                                output_modalities: [true, false, false, false],
                                set_as_default: false,
                                active_field: 0,
                            };
                        } else {
                            app.status_message = "⚠️ 请先添加提供商，再添加模型。".to_string();
                        }
                    }
                }
            } else if app.active_tab == 2 {
                let provider_names = app.sorted_provider_names();
                if provider_names.is_empty() {
                    app.status_message = "⚠️ 请先在 [2] 提供商面板添加提供商，再添加 Key。".to_string();
                } else {
                    // 默认定位到当前选中 Key 所属的提供商
                    let provider_idx = app.selected_key()
                        .and_then(|(name, _)| provider_names.iter().position(|n| *n == name))
                        .unwrap_or(0);
                    app.modal = Modal::AddKey {
                        provider_idx,
                        id: String::new(),
                        api_key: String::new(),
                        priority: "1".to_string(),
                        weight: "10".to_string(),
                        active_field: 0,
                    };
                }
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') if app.active_tab == 1 => {
            match app.tab2_focus {
                    Tab2Focus::Providers => {
                        if let Some(p_name) = app.selected_provider_name() {
                            if let Some(p) = app.config.providers.get(&p_name) {
                                let strat_idx = match p.strategy.as_str() {
                                    "priority" => 1,
                                    "weighted" => 2,
                                    _ => 0,
                                };
                                let bm_idx = billing_mode_to_provider_idx(p.billing_mode);
                                app.modal = Modal::EditProvider {
                                    name: p_name,
                                    base_url: p.base_url.clone(),
                                    default_model: p.default_model.clone(),
                                    strategy_idx: strat_idx,
                                    billing_mode_idx: bm_idx,
                                    input_price: p.input_price.to_string(),
                                    cached_price: p.cached_price.to_string(),
                                    output_price: p.output_price.to_string(),
                                    active_field: 0,
                                };
                            }
                        } else {
                            app.status_message = "⚠️ 当前没有选中的提供商可供编辑。".to_string();
                        }
                    }
                    Tab2Focus::Models => {
                        if let Some(p_name) = app.selected_provider_name() {
                            if let Some(m_cfg) = app.selected_model_config() {
                                let is_def = app.config.providers.get(&p_name)
                                    .map(|p| p.default_model == m_cfg.name)
                                    .unwrap_or(false);

                                let mut in_mods = [false; 4];
                                for (i, key) in MODALITY_KEYS.iter().enumerate() {
                                    if m_cfg.input_types.iter().any(|t| t == key) {
                                        in_mods[i] = true;
                                    }
                                }
                                let mut out_mods = [false; 4];
                                for (i, key) in MODALITY_KEYS.iter().enumerate() {
                                    if m_cfg.output_types.iter().any(|t| t == key) {
                                        out_mods[i] = true;
                                    }
                                }

                                let bm_idx = model_billing_mode_to_idx(m_cfg.billing_mode);
                                app.modal = Modal::EditModel {
                                    provider_name: p_name,
                                    model_name: m_cfg.name,
                                    tier_idx: tier_to_idx(m_cfg.tier),
                                    billing_mode_idx: bm_idx,
                                    input_price: m_cfg.input_price.map(|v| v.to_string()).unwrap_or_default(),
                                    cached_price: m_cfg.cached_price.map(|v| v.to_string()).unwrap_or_default(),
                                    output_price: m_cfg.output_price.map(|v| v.to_string()).unwrap_or_default(),
                                    context_window: m_cfg.context_window,
                                    max_output: m_cfg.max_output,
                                    input_modalities: in_mods,
                                    output_modalities: out_mods,
                                    set_as_default: is_def,
                                    active_field: 0,
                                };
                            } else {
                                app.status_message = "⚠️ 当前提供商下没有选中的模型可供编辑。".to_string();
                            }
                        } else {
                            app.status_message = "⚠️ 当前没有选中的提供商。".to_string();
                        }
                    }
                }
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if app.active_tab == 1 {
                match app.tab2_focus {
                    Tab2Focus::Providers => {
                        if let Some(p_name) = app.selected_provider_name() {
                            app.modal = Modal::DeleteProviderConfirm { name: p_name };
                        } else {
                            app.status_message = "⚠️ 当前没有选中的提供商可供删除。".to_string();
                        }
                    }
                    Tab2Focus::Models => {
                        if let Some(p_name) = app.selected_provider_name() {
                            if let Some(m_cfg) = app.selected_model_config() {
                                app.modal = Modal::DeleteModelConfirm {
                                    provider_name: p_name,
                                    model_name: m_cfg.name,
                                };
                            } else {
                                app.status_message = "⚠️ 当前没有选中的模型可供删除。".to_string();
                            }
                        } else {
                            app.status_message = "⚠️ 当前没有选中的提供商。".to_string();
                        }
                    }
                }
            } else if app.active_tab == 2 {
                if let Some((p_name, k)) = app.selected_key() {
                    app.modal = Modal::DeleteKeyConfirm {
                        provider: p_name,
                        id: k.id,
                    };
                } else {
                    app.status_message = "⚠️ 当前没有选中的 Key。".to_string();
                }
            }
        }
        KeyCode::Char('s') | KeyCode::Char('S') if app.active_tab == 1 => {
            if app.tab2_focus != Tab2Focus::Models {
                app.status_message = "👉 请先按 [Tab] 或 [→] 切换到右侧模型列表，再按 [s] 设为默认模型。".to_string();
                return;
            }
            if let Some(p_name) = app.selected_provider_name() {
                if let Some(m_cfg) = app.selected_model_config() {
                    if let Err(e) = app.config.set_default_model(&p_name, &m_cfg.name) {
                        app.status_message = format!("❌ 设置默认模型失败: {}", e);
                    } else if let Err(e) = app.save_config() {
                        app.status_message = format!("❌ 保存失败: {}", e);
                    } else {
                        app.status_message = format!("✅ 成功将 '{}' 设为 '{}' 的默认主模型", m_cfg.name, p_name);
                    }
                } else {
                    app.status_message = "⚠️ 当前没有选中的模型。".to_string();
                }
            } else {
                app.status_message = "⚠️ 当前没有选中的提供商。".to_string();
            }
        }
        _ => {}
    }
}

fn parse_modal_price(val: &str, field_name: &str) -> Result<Option<f64>, String> {
    let t = val.trim();
    if t.is_empty() {
        Ok(None)
    } else {
        match t.parse::<f64>() {
            Ok(v) if v >= 0.0 && !v.is_nan() && !v.is_infinite() => Ok(Some(v)),
            _ => Err(format!("{} 必须为大于等于 0 的有效数值，输入: '{}'", field_name, t)),
        }
    }
}

fn handle_modal_key(app: &mut TuiApp, key: KeyCode, modifiers: KeyModifiers) {
    let mut current_modal = std::mem::replace(&mut app.modal, Modal::None);
    let mut keep_modal = false;

    // 过滤有害的组合控制键
    if modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT) {
        app.modal = current_modal;
        return;
    }

    match &mut current_modal {
        Modal::None => {}
        Modal::DeleteProviderConfirm { name } => {
            match key {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let target = name.clone();
                    app.config.remove_provider(&target);
                    if let Err(e) = app.save_config() {
                        app.status_message = format!("❌ 删除后保存配置失败: {}", e);
                    } else {
                        app.status_message = format!("✅ 成功删除提供商 '{}'", target);
                    }
                    let p_len = app.config.providers.len();
                    scroll_table(&mut app.provider_table_state, p_len, 0);
                    let m_len = app.selected_models_for_current_provider().len();
                    scroll_table(&mut app.model_table_state, m_len, 0);
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.status_message = "已取消删除。".to_string();
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::DeleteModelConfirm { provider_name, model_name } => {
            match key {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let p = provider_name.clone();
                    let m = model_name.clone();
                    match app.config.remove_model(&p, &m) {
                        Ok(true) => {
                            if let Err(e) = app.save_config() {
                                app.status_message = format!("❌ 移除后保存配置失败: {}", e);
                            } else {
                                app.status_message = format!("✅ 成功从 '{}' 中移除模型 '{}'", p, m);
                            }
                            let new_m_len = app.selected_models_for_current_provider().len();
                            scroll_table(&mut app.model_table_state, new_m_len, 0);
                        }
                        Ok(false) => {
                            app.status_message = format!("⚠️ 未能移除模型 '{}'", m);
                        }
                        Err(e) => {
                            app.status_message = format!("❌ 删除失败: {}", e);
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.status_message = "已取消删除。".to_string();
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::AddProvider {
            name,
            base_url,
            default_model,
            strategy_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            active_field,
        } => {
            match key {
                KeyCode::Esc => {
                    app.status_message = "已取消添加提供商。".to_string();
                }
                KeyCode::Tab | KeyCode::Down => {
                    *active_field = (*active_field + 1) % 8;
                    keep_modal = true;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *active_field = if *active_field == 0 { 7 } else { *active_field - 1 };
                    keep_modal = true;
                }
                KeyCode::Enter => {
                    if name.trim().is_empty() {
                        app.status_message = "❌ 提供商名称不能为空！".to_string();
                        keep_modal = true;
                    } else if base_url.trim().is_empty() {
                        app.status_message = "❌ Base URL 不能为空！".to_string();
                        keep_modal = true;
                    } else if default_model.trim().is_empty() {
                        app.status_message = "❌ 默认模型名不能为空！".to_string();
                        keep_modal = true;
                    } else {
                        let in_p = match parse_modal_price(input_price, "常规输入单价") {
                            Ok(v) => v.unwrap_or(0.50),
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let ca_p = match parse_modal_price(cached_price, "缓存命中单价") {
                            Ok(v) => v.unwrap_or(0.25),
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let out_p = match parse_modal_price(output_price, "输出生成单价") {
                            Ok(v) => v.unwrap_or(1.00),
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let strat = STRATEGIES[*strategy_idx];
                        let bm = idx_to_provider_billing_mode(*billing_mode_idx);
                        let p_name = name.trim().to_string();
                        app.config.add_provider(&p_name, base_url.trim(), default_model.trim(), strat);
                        if let Some(p) = app.config.providers.get_mut(&p_name) {
                            p.billing_mode = bm;
                            p.input_price = in_p;
                            p.cached_price = ca_p;
                            p.output_price = out_p;
                        }
                        if let Err(e) = app.save_config() {
                            app.status_message = format!("❌ 保存失败: {}", e);
                        } else {
                            app.status_message = format!("✅ 成功添加提供商 '{}' [模式: {:?}]", p_name, bm);
                            let len = app.config.providers.len();
                            scroll_table(&mut app.provider_table_state, len, 0);
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    match *active_field {
                        0 => name.push(' '),
                        1 => base_url.push(' '),
                        2 => default_model.push(' '),
                        3 => *strategy_idx = (*strategy_idx + 1) % 3,
                        4 => *billing_mode_idx = (*billing_mode_idx + 1) % 3,
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Left => {
                    if *active_field == 3 && *strategy_idx > 0 {
                        *strategy_idx -= 1;
                    } else if *active_field == 4 && *billing_mode_idx > 0 {
                        *billing_mode_idx -= 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Right => {
                    if *active_field == 3 && *strategy_idx < 2 {
                        *strategy_idx += 1;
                    } else if *active_field == 4 && *billing_mode_idx < 2 {
                        *billing_mode_idx += 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Char('1') if *active_field == 3 => { *strategy_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 3 => { *strategy_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 3 => { *strategy_idx = 2; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 4 => { *billing_mode_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 4 => { *billing_mode_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 4 => { *billing_mode_idx = 2; keep_modal = true; }
                KeyCode::Backspace => {
                    match *active_field {
                        0 => { name.pop(); }
                        1 => { base_url.pop(); }
                        2 => { default_model.pop(); }
                        5 => { input_price.pop(); }
                        6 => { cached_price.pop(); }
                        7 => { output_price.pop(); }
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Char(c) => {
                    match *active_field {
                        0 => name.push(c),
                        1 => base_url.push(c),
                        2 => default_model.push(c),
                        5 if c.is_ascii_digit() || c == '.' => input_price.push(c),
                        6 if c.is_ascii_digit() || c == '.' => cached_price.push(c),
                        7 if c.is_ascii_digit() || c == '.' => output_price.push(c),
                        _ => {}
                    }
                    keep_modal = true;
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::EditProvider {
            name,
            base_url,
            default_model,
            strategy_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            active_field,
        } => {
            match key {
                KeyCode::Esc => {
                    app.status_message = "已取消编辑。".to_string();
                }
                KeyCode::Tab | KeyCode::Down => {
                    *active_field = (*active_field + 1) % 7;
                    keep_modal = true;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *active_field = if *active_field == 0 { 6 } else { *active_field - 1 };
                    keep_modal = true;
                }
                KeyCode::Enter => {
                    if base_url.trim().is_empty() {
                        app.status_message = "❌ Base URL 不能为空！".to_string();
                        keep_modal = true;
                    } else if default_model.trim().is_empty() {
                        app.status_message = "❌ 默认模型名不能为空！".to_string();
                        keep_modal = true;
                    } else {
                        let in_p = match parse_modal_price(input_price, "常规输入单价") {
                            Ok(v) => v.unwrap_or(0.50),
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let ca_p = match parse_modal_price(cached_price, "缓存命中单价") {
                            Ok(v) => v.unwrap_or(0.25),
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let out_p = match parse_modal_price(output_price, "输出生成单价") {
                            Ok(v) => v.unwrap_or(1.00),
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let strat = STRATEGIES[*strategy_idx];
                        let bm = idx_to_provider_billing_mode(*billing_mode_idx);
                        let name_str = name.clone();
                        if let Err(e) = app.config.update_provider(&name_str, base_url.trim(), default_model.trim(), strat) {
                            app.status_message = format!("❌ 更新提供商失败: {}", e);
                        } else {
                            if let Some(p) = app.config.providers.get_mut(&name_str) {
                                p.billing_mode = bm;
                                p.input_price = in_p;
                                p.cached_price = ca_p;
                                p.output_price = out_p;
                            }
                            if let Err(e) = app.save_config() {
                                app.status_message = format!("❌ 保存配置失败: {}", e);
                            } else {
                                app.status_message = format!("✅ 成功更新提供商 '{}' [模式: {:?}]", name_str, bm);
                            }
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    match *active_field {
                        0 => base_url.push(' '),
                        1 => default_model.push(' '),
                        2 => *strategy_idx = (*strategy_idx + 1) % 3,
                        3 => *billing_mode_idx = (*billing_mode_idx + 1) % 3,
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Left => {
                    if *active_field == 2 && *strategy_idx > 0 {
                        *strategy_idx -= 1;
                    } else if *active_field == 3 && *billing_mode_idx > 0 {
                        *billing_mode_idx -= 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Right => {
                    if *active_field == 2 && *strategy_idx < 2 {
                        *strategy_idx += 1;
                    } else if *active_field == 3 && *billing_mode_idx < 2 {
                        *billing_mode_idx += 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Char('1') if *active_field == 2 => { *strategy_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 2 => { *strategy_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 2 => { *strategy_idx = 2; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 3 => { *billing_mode_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 3 => { *billing_mode_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 3 => { *billing_mode_idx = 2; keep_modal = true; }
                KeyCode::Backspace => {
                    match *active_field {
                        0 => { base_url.pop(); }
                        1 => { default_model.pop(); }
                        4 => { input_price.pop(); }
                        5 => { cached_price.pop(); }
                        6 => { output_price.pop(); }
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Char(c) => {
                    match *active_field {
                        0 => base_url.push(c),
                        1 => default_model.push(c),
                        4 if c.is_ascii_digit() || c == '.' => input_price.push(c),
                        5 if c.is_ascii_digit() || c == '.' => cached_price.push(c),
                        6 if c.is_ascii_digit() || c == '.' => output_price.push(c),
                        _ => {}
                    }
                    keep_modal = true;
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::AddModel {
            provider_name,
            model_name,
            tier_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            context_window,
            max_output,
            input_modalities,
            output_modalities,
            set_as_default,
            active_field,
        } => {
            match key {
                KeyCode::Esc => {
                    app.status_message = "已取消添加模型。".to_string();
                }
                KeyCode::Tab | KeyCode::Down => {
                    *active_field = (*active_field + 1) % 11;
                    keep_modal = true;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *active_field = if *active_field == 0 { 10 } else { *active_field - 1 };
                    keep_modal = true;
                }
                KeyCode::Enter => {
                    if model_name.trim().is_empty() {
                        app.status_message = "❌ 模型名称不能为空！".to_string();
                        keep_modal = true;
                    } else {
                        let in_types: Vec<String> = input_modalities.iter().enumerate()
                            .filter(|(_, &on)| on)
                            .map(|(i, _)| MODALITY_KEYS[i].to_string())
                            .collect();
                        let out_types: Vec<String> = output_modalities.iter().enumerate()
                            .filter(|(_, &on)| on)
                            .map(|(i, _)| MODALITY_KEYS[i].to_string())
                            .collect();

                        let m_name = model_name.trim().to_string();
                        let in_p = match parse_modal_price(input_price, "常规输入单价") {
                            Ok(v) => v,
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let ca_p = match parse_modal_price(cached_price, "缓存命中单价") {
                            Ok(v) => v,
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let out_p = match parse_modal_price(output_price, "输出生成单价") {
                            Ok(v) => v,
                            Err(e) => {
                                app.status_message = format!("❌ {}", e);
                                app.modal = current_modal;
                                return;
                            }
                        };
                        let tier_val = TIERS[*tier_idx];
                        let mode_val = idx_to_model_billing_mode(*billing_mode_idx);

                        let cfg = ModelConfig {
                            name: m_name.clone(),
                            tier: tier_val,
                            context_window: if context_window.trim().is_empty() { "128K".to_string() } else { context_window.trim().to_string() },
                            max_output: if max_output.trim().is_empty() { "4K".to_string() } else { max_output.trim().to_string() },
                            input_types: if in_types.is_empty() { vec!["text".to_string()] } else { in_types },
                            output_types: if out_types.is_empty() { vec!["text".to_string()] } else { out_types },
                            billing_mode: mode_val,
                            input_price: in_p,
                            cached_price: ca_p,
                            output_price: out_p,
                        };

                        let p_name = provider_name.clone();
                        let is_def = *set_as_default;

                        if let Err(e) = app.config.upsert_model_config(&p_name, cfg) {
                            app.status_message = format!("❌ 添加模型配置失败: {}", e);
                        } else {
                            if is_def {
                                let _ = app.config.set_default_model(&p_name, &m_name);
                            }
                            if let Err(e) = app.save_config() {
                                app.status_message = format!("❌ 保存失败: {}", e);
                            } else {
                                app.status_message = format!("✅ 成功向 '{}' 添加模型 '{}' [{}]", p_name, m_name, tier_val.shorthand());
                                let m_len = app.selected_models_for_current_provider().len();
                                scroll_table(&mut app.model_table_state, m_len, 0);
                            }
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    match *active_field {
                        0 => model_name.push(' '),
                        1 => *tier_idx = (*tier_idx + 1) % 3,
                        2 => *billing_mode_idx = (*billing_mode_idx + 1) % 4,
                        6 => context_window.push(' '),
                        7 => max_output.push(' '),
                        8 => input_modalities[0] = !input_modalities[0],
                        9 => output_modalities[0] = !output_modalities[0],
                        10 => *set_as_default = !*set_as_default,
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Left => {
                    if *active_field == 1 && *tier_idx > 0 {
                        *tier_idx -= 1;
                    } else if *active_field == 2 && *billing_mode_idx > 0 {
                        *billing_mode_idx -= 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Right => {
                    if *active_field == 1 && *tier_idx < 2 {
                        *tier_idx += 1;
                    } else if *active_field == 2 && *billing_mode_idx < 3 {
                        *billing_mode_idx += 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Char('1') if *active_field == 1 => { *tier_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 1 => { *tier_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 1 => { *tier_idx = 2; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 2 => { *billing_mode_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 2 => { *billing_mode_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 2 => { *billing_mode_idx = 2; keep_modal = true; }
                KeyCode::Char('4') if *active_field == 2 => { *billing_mode_idx = 3; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 8 => { input_modalities[0] = !input_modalities[0]; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 8 => { input_modalities[1] = !input_modalities[1]; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 8 => { input_modalities[2] = !input_modalities[2]; keep_modal = true; }
                KeyCode::Char('4') if *active_field == 8 => { input_modalities[3] = !input_modalities[3]; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 9 => { output_modalities[0] = !output_modalities[0]; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 9 => { output_modalities[1] = !output_modalities[1]; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 9 => { output_modalities[2] = !output_modalities[2]; keep_modal = true; }
                KeyCode::Char('4') if *active_field == 9 => { output_modalities[3] = !output_modalities[3]; keep_modal = true; }
                KeyCode::Backspace => {
                    match *active_field {
                        0 => { model_name.pop(); }
                        3 => { input_price.pop(); }
                        4 => { cached_price.pop(); }
                        5 => { output_price.pop(); }
                        6 => { context_window.pop(); }
                        7 => { max_output.pop(); }
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Char(c) => {
                    match *active_field {
                        0 => model_name.push(c),
                        3 if c.is_ascii_digit() || c == '.' => input_price.push(c),
                        4 if c.is_ascii_digit() || c == '.' => cached_price.push(c),
                        5 if c.is_ascii_digit() || c == '.' => output_price.push(c),
                        6 => context_window.push(c),
                        7 => max_output.push(c),
                        _ => {}
                    }
                    keep_modal = true;
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::EditModel {
            provider_name,
            model_name,
            tier_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            context_window,
            max_output,
            input_modalities,
            output_modalities,
            set_as_default,
            active_field,
        } => {
            match key {
                KeyCode::Esc => {
                    app.status_message = "已取消编辑。".to_string();
                }
                KeyCode::Tab | KeyCode::Down => {
                    *active_field = (*active_field + 1) % 10;
                    keep_modal = true;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *active_field = if *active_field == 0 { 9 } else { *active_field - 1 };
                    keep_modal = true;
                }
                KeyCode::Enter => {
                    let in_types: Vec<String> = input_modalities.iter().enumerate()
                        .filter(|(_, &on)| on)
                        .map(|(i, _)| MODALITY_KEYS[i].to_string())
                        .collect();
                    let out_types: Vec<String> = output_modalities.iter().enumerate()
                        .filter(|(_, &on)| on)
                        .map(|(i, _)| MODALITY_KEYS[i].to_string())
                        .collect();

                    let in_p = match parse_modal_price(input_price, "常规输入单价") {
                        Ok(v) => v,
                        Err(e) => {
                            app.status_message = format!("❌ {}", e);
                            app.modal = current_modal;
                            return;
                        }
                    };
                    let ca_p = match parse_modal_price(cached_price, "缓存命中单价") {
                        Ok(v) => v,
                        Err(e) => {
                            app.status_message = format!("❌ {}", e);
                            app.modal = current_modal;
                            return;
                        }
                    };
                    let out_p = match parse_modal_price(output_price, "输出生成单价") {
                        Ok(v) => v,
                        Err(e) => {
                            app.status_message = format!("❌ {}", e);
                            app.modal = current_modal;
                            return;
                        }
                    };
                    let tier_val = TIERS[*tier_idx];
                    let mode_val = idx_to_model_billing_mode(*billing_mode_idx);

                    let cfg = ModelConfig {
                        name: model_name.clone(),
                        tier: tier_val,
                        context_window: if context_window.trim().is_empty() { "128K".to_string() } else { context_window.trim().to_string() },
                        max_output: if max_output.trim().is_empty() { "4K".to_string() } else { max_output.trim().to_string() },
                        input_types: if in_types.is_empty() { vec!["text".to_string()] } else { in_types },
                        output_types: if out_types.is_empty() { vec!["text".to_string()] } else { out_types },
                        billing_mode: mode_val,
                        input_price: in_p,
                        cached_price: ca_p,
                        output_price: out_p,
                    };

                    let p_name = provider_name.clone();
                    let m_name = model_name.clone();
                    let is_def = *set_as_default;

                    if let Err(e) = app.config.upsert_model_config(&p_name, cfg) {
                        app.status_message = format!("❌ 更新模型配置失败: {}", e);
                    } else {
                        if is_def {
                            let _ = app.config.set_default_model(&p_name, &m_name);
                        }
                        if let Err(e) = app.save_config() {
                            app.status_message = format!("❌ 保存失败: {}", e);
                        } else {
                            app.status_message = format!("✅ 成功更新模型 '{}' 参数配置 [{}]", m_name, tier_val.shorthand());
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    match *active_field {
                        0 => *tier_idx = (*tier_idx + 1) % 3,
                        1 => *billing_mode_idx = (*billing_mode_idx + 1) % 4,
                        5 => context_window.push(' '),
                        6 => max_output.push(' '),
                        7 => input_modalities[0] = !input_modalities[0],
                        8 => output_modalities[0] = !output_modalities[0],
                        9 => *set_as_default = !*set_as_default,
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Left => {
                    if *active_field == 0 && *tier_idx > 0 {
                        *tier_idx -= 1;
                    } else if *active_field == 1 && *billing_mode_idx > 0 {
                        *billing_mode_idx -= 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Right => {
                    if *active_field == 0 && *tier_idx < 2 {
                        *tier_idx += 1;
                    } else if *active_field == 1 && *billing_mode_idx < 3 {
                        *billing_mode_idx += 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Char('1') if *active_field == 0 => { *tier_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 0 => { *tier_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 0 => { *tier_idx = 2; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 1 => { *billing_mode_idx = 0; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 1 => { *billing_mode_idx = 1; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 1 => { *billing_mode_idx = 2; keep_modal = true; }
                KeyCode::Char('4') if *active_field == 1 => { *billing_mode_idx = 3; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 7 => { input_modalities[0] = !input_modalities[0]; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 7 => { input_modalities[1] = !input_modalities[1]; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 7 => { input_modalities[2] = !input_modalities[2]; keep_modal = true; }
                KeyCode::Char('4') if *active_field == 7 => { input_modalities[3] = !input_modalities[3]; keep_modal = true; }
                KeyCode::Char('1') if *active_field == 8 => { output_modalities[0] = !output_modalities[0]; keep_modal = true; }
                KeyCode::Char('2') if *active_field == 8 => { output_modalities[1] = !output_modalities[1]; keep_modal = true; }
                KeyCode::Char('3') if *active_field == 8 => { output_modalities[2] = !output_modalities[2]; keep_modal = true; }
                KeyCode::Char('4') if *active_field == 8 => { output_modalities[3] = !output_modalities[3]; keep_modal = true; }
                KeyCode::Backspace => {
                    match *active_field {
                        2 => { input_price.pop(); }
                        3 => { cached_price.pop(); }
                        4 => { output_price.pop(); }
                        5 => { context_window.pop(); }
                        6 => { max_output.pop(); }
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Char(c) => {
                    match *active_field {
                        2 if c.is_ascii_digit() || c == '.' => input_price.push(c),
                        3 if c.is_ascii_digit() || c == '.' => cached_price.push(c),
                        4 if c.is_ascii_digit() || c == '.' => output_price.push(c),
                        5 => context_window.push(c),
                        6 => max_output.push(c),
                        _ => {}
                    }
                    keep_modal = true;
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::DeleteKeyConfirm { provider, id } => {
            match key {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let p = provider.clone();
                    let k = id.clone();
                    match app.config.remove_key(&p, &k) {
                        Ok(true) => {
                            match app.save_config() {
                                Ok(_) => {
                                    app.status_message = format!("✅ 成功从提供商 '{}' 中删除 Key '{}'", p, k);
                                }
                                Err(e) => {
                                    app.status_message = format!("⚠️ 已从内存删除 Key '{}'，但写入配置文件失败: {} (重启后可能恢复)", k, e);
                                }
                            }
                            let total_keys = app.config.providers.values().map(|x| x.keys.len()).sum();
                            scroll_table(&mut app.key_table_state, total_keys, 0);
                        }
                        Ok(false) => {
                            app.status_message = format!("⚠️ 未能删除 Key '{}'", k);
                        }
                        Err(e) => {
                            app.status_message = format!("❌ 删除失败: {}", e);
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.status_message = "已取消删除 Key。".to_string();
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
        Modal::AddKey { provider_idx, id, api_key, priority, weight, active_field } => {
            match key {
                KeyCode::Esc => {
                    app.status_message = "已取消添加 Key。".to_string();
                }
                KeyCode::Tab | KeyCode::Down => {
                    *active_field = (*active_field + 1) % 5;
                    keep_modal = true;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    *active_field = if *active_field == 0 { 4 } else { *active_field - 1 };
                    keep_modal = true;
                }
                KeyCode::Left => {
                    if *active_field == 0 && *provider_idx > 0 {
                        *provider_idx -= 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Right => {
                    if *active_field == 0 && *provider_idx + 1 < app.sorted_provider_names().len() {
                        *provider_idx += 1;
                    }
                    keep_modal = true;
                }
                KeyCode::Enter => {
                    let provider_names = app.sorted_provider_names();
                    let prio_res: Result<u32, _> = priority.trim().parse();
                    let weight_res: Result<u32, _> = weight.trim().parse();
                    if provider_names.is_empty() {
                        app.status_message = "❌ 无可用的提供商，请先添加提供商。".to_string();
                        keep_modal = true;
                    } else if id.trim().is_empty() {
                        app.status_message = "❌ Key ID 不能为空！".to_string();
                        keep_modal = true;
                    } else if api_key.trim().is_empty() {
                        app.status_message = "❌ API Key 内容不能为空！".to_string();
                        keep_modal = true;
                    } else if prio_res.is_err() {
                        app.status_message = "❌ 优先级必须是非负整数 (1 为最高)。".to_string();
                        keep_modal = true;
                    } else if weight_res.is_err() {
                        app.status_message = "❌ 权重必须是非负整数。".to_string();
                        keep_modal = true;
                    } else {
                        let p_name = provider_names[*provider_idx % provider_names.len()].clone();
                        let prio: u32 = prio_res.unwrap_or(1);
                        let w: u32 = weight_res.unwrap_or(10);
                        if let Err(e) = app.config.add_key(&p_name, id.trim(), api_key.trim(), prio, w) {
                            app.status_message = format!("❌ 添加 Key 失败: {}", e);
                            keep_modal = true;
                        } else if let Err(e) = app.save_config() {
                            app.status_message = format!("❌ 保存失败: {}", e);
                            keep_modal = true;
                        } else {
                            app.status_message = format!("✅ 成功向提供商 '{}' 添加/更新 Key '{}' (优先级:{}, 权重:{})", p_name, id.trim(), prio, w);
                            let total_keys = app.config.providers.values().map(|x| x.keys.len()).sum();
                            scroll_table(&mut app.key_table_state, total_keys, 0);
                        }
                    }
                }
                KeyCode::Backspace => {
                    match *active_field {
                        1 => { id.pop(); }
                        2 => { api_key.pop(); }
                        3 => { priority.pop(); }
                        4 => { weight.pop(); }
                        _ => {}
                    }
                    keep_modal = true;
                }
                KeyCode::Char(c) => {
                    match *active_field {
                        1 => id.push(c),
                        2 => api_key.push(c),
                        3 => priority.push(c),
                        4 => weight.push(c),
                        _ => {}
                    }
                    keep_modal = true;
                }
                _ => {
                    keep_modal = true;
                }
            }
        }
    }

    if keep_modal {
        app.modal = current_modal;
    }
}

fn scroll_table(state: &mut TableState, max_len: usize, delta: isize) {
    if max_len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, (max_len - 1) as isize) as usize;
    state.select(Some(next));
}

fn ui(f: &mut Frame, app: &mut TuiApp) {
    let area = f.area();
    if area.width < 60 || area.height < 12 {
        let warn_msg = Paragraph::new(format!(
            "⚠️ 终端窗口过小 (当前: {}x{})，请调整至至少 60x12 以正常显示看板。",
            area.width, area.height
        ))
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
        f.render_widget(warn_msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // 极简 Header Tabs (单行+下细线)
            Constraint::Min(0),    // 主内容区
            Constraint::Length(2), // 极简 Footer (状态与快捷键提示)
        ])
        .split(area);

    // 1. Header (极简无大框导航)
    render_header(f, chunks[0], app);

    // 2. Tab Contents
    match app.active_tab {
        0 => render_overview_tab(f, chunks[1], app),
        1 => render_providers_and_models_tab(f, chunks[1], app),
        2 => render_keys_tab(f, chunks[1], app),
        3 => render_telemetry_tab(f, chunks[1], app),
        _ => {}
    }

    // 3. Footer (极简状态栏)
    render_footer(f, chunks[2], app);

    // 4. Modal Dialog (如果有)
    render_modal(f, area, app);
}

fn render_header(f: &mut Frame, area: Rect, app: &TuiApp) {
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(26)])
        .split(area);

    let tab_titles = vec![
        " 1 概览 ",
        " 2 提供商与模型 ",
        " 3 Key 治理 ",
        " 4 故障录波 ",
    ];

    let tabs = Tabs::new(tab_titles)
        .select(app.active_tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
                .bg(Color::Rgb(25, 35, 45)),
        )
        .divider("│");
    f.render_widget(tabs, header_chunks[0]);

    // 网关状态
    let (status_icon, status_text, status_color) = if app.is_online {
        ("●", "网关在线 UP", Color::Green)
    } else {
        ("○", "网关离线 DOWN", Color::Yellow)
    };

    let status_widget = Paragraph::new(Line::from(vec![
        Span::styled(format!("{} ", status_icon), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::raw(" "),
    ]))
    .alignment(Alignment::Right);
    f.render_widget(status_widget, header_chunks[1]);
}

fn render_footer(f: &mut Frame, area: Rect, app: &TuiApp) {
    let help_line = if app.active_tab == 1 {
        match app.tab2_focus {
            Tab2Focus::Providers => {
                Line::from(vec![
                    Span::styled(" [Tab/→] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw("切到模型  "),
                    Span::styled(" [a] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw("新建提供商  "),
                    Span::styled(" [e] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw("编辑  "),
                    Span::styled(" [d] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("删除  "),
                    Span::styled(" [j/k/↑/↓] ", Style::default().fg(Color::DarkGray)),
                    Span::raw("移动  "),
                    Span::styled(" [q] ", Style::default().fg(Color::DarkGray)),
                    Span::raw("退出"),
                ])
            }
            Tab2Focus::Models => {
                Line::from(vec![
                    Span::styled(" [Tab/←] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw("切回提供商  "),
                    Span::styled(" [a] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw("添加模型  "),
                    Span::styled(" [e] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw("编辑参数  "),
                    Span::styled(" [s] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("设为默认  "),
                    Span::styled(" [d] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("删除  "),
                    Span::styled(" [q] ", Style::default().fg(Color::DarkGray)),
                    Span::raw("退出"),
                ])
            }
        }
    } else if app.active_tab == 2 {
        Line::from(vec![
            Span::styled(" [a] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("添加Key  "),
            Span::styled(" [d] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw("删除  "),
            Span::styled(" [j/k/↑/↓] ", Style::default().fg(Color::DarkGray)),
            Span::raw("移动  "),
            Span::styled(" [Tab] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("切页  "),
            Span::styled(" [q] ", Style::default().fg(Color::DarkGray)),
            Span::raw("退出"),
        ])
    } else {
        Line::from(vec![
            Span::styled(" [Tab] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("切页  "),
            Span::styled(" [1-4] ", Style::default().fg(Color::Yellow)),
            Span::raw("直达  "),
            Span::styled(" [j/k/↑/↓] ", Style::default().fg(Color::DarkGray)),
            Span::raw("移动  "),
            Span::styled(" [r] ", Style::default().fg(Color::DarkGray)),
            Span::raw("刷新  "),
            Span::styled(" [q/Esc] ", Style::default().fg(Color::DarkGray)),
            Span::raw("退出"),
        ])
    };

    let status_line = Line::from(vec![
        Span::styled("  › ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(&app.status_message, Style::default().fg(Color::White)),
    ]);

    let footer = Paragraph::new(vec![help_line, status_line])
        .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::Rgb(50, 60, 75))));
    f.render_widget(footer, area);
}

fn render_overview_tab(f: &mut Frame, area: Rect, app: &TuiApp) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(area);

    let top_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(vertical[0]);

    // Cards
    let total_reqs = app.metrics.as_ref()
        .and_then(|m| m.get("total_requests").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let total_success = app.metrics.as_ref()
        .and_then(|m| m.get("successful_requests").or_else(|| m.get("total_success")).and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let total_failover = app.metrics.as_ref()
        .and_then(|m| m.get("total_failover").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let total_errors = app.metrics.as_ref()
        .and_then(|m| m.get("failed_requests").or_else(|| m.get("total_errors")).and_then(|v| v.as_u64()))
        .unwrap_or(0);

    let card1 = Paragraph::new(vec![
        Line::from(Span::styled("网关监听地址", Style::default().fg(Color::Gray))),
        Line::from(Span::styled(&app.config.gateway.bind, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(format!("服务: {}", if app.is_online { "🟢 在线运行" } else { "🔴 离线未连通" })),
    ])
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(card1, top_row[0]);

    let card2 = Paragraph::new(vec![
        Line::from(Span::styled("总请求流量", Style::default().fg(Color::Gray))),
        Line::from(Span::styled(format!("{} 次", total_reqs), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))),
        Line::from(format!("成功调用: {} 次", total_success)),
    ])
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(card2, top_row[1]);

    let card3 = Paragraph::new(vec![
        Line::from(Span::styled("429 / 故障自动倒换", Style::default().fg(Color::Gray))),
        Line::from(Span::styled(format!("{} 次", total_failover), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("无感毫秒级熔断倒换"),
    ])
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(card3, top_row[2]);

    let card4 = Paragraph::new(vec![
        Line::from(Span::styled("不可恢复异常 (5xx)", Style::default().fg(Color::Gray))),
        Line::from(Span::styled(format!("{} 次", total_errors), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
        Line::from("全链路故障录波保障"),
    ])
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(card4, top_row[3]);

    // Bottom Half
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[1]);

    // Provider summary table
    let sorted_names = app.sorted_provider_names();
    let prov_rows: Vec<Row> = sorted_names.iter().map(|name| {
        if let Some(p) = app.config.providers.get(name) {
            Row::new(vec![
                Cell::from(name.as_str()),
                Cell::from(p.default_model.as_str()),
                Cell::from(p.strategy.as_str()),
                Cell::from(p.list_all_models().len().to_string()),
                Cell::from(p.keys.len().to_string()),
            ])
        } else {
            Row::new(vec![Cell::from(name.as_str())])
        }
    }).collect();

    let prov_table = Table::new(
        prov_rows,
        [
            Constraint::Percentage(22),
            Constraint::Percentage(32),
            Constraint::Percentage(22),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
        ],
    )
    .header(
        Row::new(vec!["提供商", "默认主模型", "调度策略", "模型数", "Key数"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    )
    .block(
        Block::default()
            .borders(Borders::TOP)
            .title(" ── 已挂载模型提供商 ────────────────────────────────────────── ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(prov_table, bottom_chunks[0]);

    // Quickstart info -> 流速遥测资产面板（复用 /v1/telemetry/stream）
    let (stream_line1, stream_line2, provider_lines) = build_stream_lines(app);
    let mut info_lines = vec![
        Line::from(Span::styled("流速遥测 TTFT/TPOT/stall/TTLB:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        stream_line1,
        stream_line2,
        Line::from(""),
        Line::from(Span::styled("分提供商对比（TTFT/TPS/stall）:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
    ];
    info_lines.extend(provider_lines);
    info_lines.push(Line::from(""));
    info_lines.push(Line::from("  • RTT型慢看TTFT偏移；攒包型卡看p95/stall，[4]黑匣子看单帧stream_flow"));
    let info = Paragraph::new(info_lines)
    .block(
        Block::default()
            .borders(Borders::TOP)
            .title(" ── 流速遥测与A/B对比 ─────────────────────────────────────────── ")
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .wrap(Wrap { trim: true });
    f.render_widget(info, bottom_chunks[1]);
}

fn metric_f64(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| x.as_f64().or_else(|| x.as_u64().map(|n| n as f64)))
}

fn build_stream_lines(app: &TuiApp) -> (Line<'static>, Line<'static>, Vec<Line<'static>>) {
    let global = app.stream.as_ref().and_then(|s| s.get("global"));
    let avg_ttft = metric_f64(global.and_then(|g| g.get("avg_ttft_ms")));
    let avg_ttlb = metric_f64(global.and_then(|g| g.get("avg_ttlb_ms")));
    let max_gap = metric_f64(global.and_then(|g| g.get("max_gap_ms")));
    let avg_tps = metric_f64(global.and_then(|g| g.get("avg_tps")));
    let stream_count = global.and_then(|g| g.get("stream_count")).and_then(|v| v.as_u64()).unwrap_or(0);
    let total_stalls = global.and_then(|g| g.get("total_stalls")).and_then(|v| v.as_u64()).unwrap_or(0);
    let fmt_ms = |v: Option<f64>| match v {
        Some(n) => format!("{:.0}ms", n),
        None => "-".to_string(),
    };
    let l1 = Line::from(format!(
        "  全局流 {} 条 │ TTFT均值 {} │ TTLB均值 {} │ TPS均值 {}",
        stream_count,
        fmt_ms(avg_ttft),
        fmt_ms(avg_ttlb),
        avg_tps.map(|n| format!("{:.1}", n)).unwrap_or_else(|| "-".to_string()),
    ));
    let l2 = Line::from(format!(
        "  stall(>1s)累计 {} 次 │ 最大间隔 {} │ 遥测丢数 {} │ 判读：TTFT差常数=RTT多跳，stall多=缓冲攒包",
        total_stalls,
        fmt_ms(max_gap),
        app.stream.as_ref().and_then(|s| s.get("dropped")).and_then(|v| v.as_u64()).unwrap_or(0),
    ));
    let mut rows: Vec<(String, f64, f64, u64)> = Vec::new();
    if let Some(providers) = app.stream.as_ref().and_then(|s| s.get("providers")).and_then(|v| v.as_object()) {
        for (name, snap) in providers {
            let ttft = metric_f64(snap.get("ttft_ms")).unwrap_or(0.0);
            let tps = metric_f64(snap.get("tps")).unwrap_or(0.0);
            let stalls = snap.get("total_stalls").and_then(|v| v.as_u64()).unwrap_or(0);
            rows.push((name.clone(), ttft, tps, stalls));
        }
    }
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(Line::from("  暂无流样本（发起一次 stream:true 请求后自动产出）"));
    } else {
        for (name, ttft, tps, stalls) in rows.into_iter().take(6) {
            lines.push(Line::from(format!("  • {} TTFT {:.0}ms TPS {:.1} stall {}", name, ttft, tps, stalls)));
        }
    }
    (l1, l2, lines)
}

fn render_providers_and_models_tab(f: &mut Frame, area: Rect, app: &mut TuiApp) {
    let split_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    let is_p_active = app.tab2_focus == Tab2Focus::Providers;
    let is_m_active = app.tab2_focus == Tab2Focus::Models;

    // ── Left: Provider List ──
    let sorted_names = app.sorted_provider_names();
    let selected_p_idx = app.provider_table_state.selected().unwrap_or(0);

    let p_rows: Vec<Row> = sorted_names.iter().enumerate().map(|(i, name)| {
        let is_sel = i == selected_p_idx;
        let prefix = if is_sel { "› " } else { "  " };

        let style = if is_sel && is_p_active {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if is_sel {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        if let Some(p) = app.config.providers.get(name) {
            let mode_tag = match p.billing_mode {
                ponyllm_core::pool::BillingMode::Plan => "[Plan]",
                ponyllm_core::pool::BillingMode::Metered => "[按量]",
                ponyllm_core::pool::BillingMode::Free => "[免费]",
            };
            Row::new(vec![
                Cell::from(format!("{}{}", prefix, name)),
                Cell::from(format!("{} {}", p.strategy.as_str(), mode_tag)),
                Cell::from(p.keys.len().to_string()),
            ]).style(style)
        } else {
            Row::new(vec![Cell::from(format!("{}{}", prefix, name))]).style(style)
        }
    }).collect();

    let p_header_style = if is_p_active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)
    };

    let p_border_style = if is_p_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let p_table = Table::new(
        p_rows,
        [
            Constraint::Percentage(48),
            Constraint::Percentage(34),
            Constraint::Percentage(18),
        ],
    )
    .header(Row::new(vec!["提供商 (Provider)", "策略/模式", "Keys"]).style(p_header_style))
    .block(
        Block::default()
            .borders(Borders::RIGHT | Borders::TOP)
            .title(format!(" ── 提供商列表 (共 {} 个) ── ", app.config.providers.len()))
            .border_style(p_border_style),
    );

    f.render_stateful_widget(p_table, split_chunks[0], &mut app.provider_table_state);

    // ── Right: Models Split (Top: Table, Bottom: Spec Card) ──
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(split_chunks[1]);

    let p_name = app.selected_provider_name();
    let current_p = p_name.as_ref().and_then(|n| app.config.providers.get(n));
    let default_model_name = current_p.map(|p| p.default_model.as_str()).unwrap_or("");

    let models = app.selected_models_for_current_provider();
    let selected_m_idx = app.model_table_state.selected().unwrap_or(0);

    let m_rows: Vec<Row> = models.iter().enumerate().map(|(i, m)| {
        let is_def = m.name == default_model_name;
        let is_sel = i == selected_m_idx;
        let prefix = if is_sel { "› " } else { "  " };
        let name_display = if is_def {
            format!("{}{} ★", prefix, m.name)
        } else {
            format!("{}{}", prefix, m.name)
        };

        let style = if is_sel && is_m_active {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if is_sel {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let pricing_display = if let Some(p) = current_p {
            if m.input_price.is_some() || m.cached_price.is_some() || m.output_price.is_some() {
                let pr = p.get_model_pricing(&m.name);
                format!("★ {:.2}/{:.3}/{:.2}", pr.input_price, pr.cached_price, pr.output_price)
            } else {
                let pr = p.pricing();
                format!("{:.2}/{:.3}/{:.2}*", pr.input_price, pr.cached_price, pr.output_price)
            }
        } else {
            "-".to_string()
        };

        let mode_tag = if let Some(p) = current_p {
            match p.get_model_billing_mode(&m.name) {
                ponyllm_core::pool::BillingMode::Plan => "[Plan]",
                ponyllm_core::pool::BillingMode::Metered => "[按量]",
                ponyllm_core::pool::BillingMode::Free => "[免费]",
            }
        } else {
            ""
        };
        let tier_display = format!("{} {}", m.tier.shorthand(), mode_tag);

        Row::new(vec![
            Cell::from(name_display),
            Cell::from(tier_display),
            Cell::from(m.context_window.as_str()),
            Cell::from(m.max_output.as_str()),
            Cell::from(pricing_display),
        ]).style(style)
    }).collect();

    let m_header_style = if is_m_active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)
    };

    let m_border_style = if is_m_active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title_name = p_name.as_deref().unwrap_or("未选择");
    let m_table = Table::new(
        m_rows,
        [
            Constraint::Percentage(26),
            Constraint::Percentage(14),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(30),
        ],
    )
    .header(Row::new(vec!["模型标识 (Model)", "梯队/模式", "上下文", "最大输出", "资费($/1M:入/缓/出)"]).style(m_header_style))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .title(format!(" ── 模型参数矩阵 [{}] (共 {} 个模型) ── ", title_name, models.len()))
            .border_style(m_border_style),
    );

    f.render_stateful_widget(m_table, right_chunks[0], &mut app.model_table_state);

    // Selected Model Spec Card
    if let Some(m_cfg) = app.selected_model_config() {
        let is_def = current_p.map(|p| p.default_model == m_cfg.name).unwrap_or(false);
        let base_url = current_p.map(|p| p.base_url.as_str()).unwrap_or("-");

        let in_tags: Vec<Span> = m_cfg.input_types.iter().map(|t| {
            Span::styled(format!(" [入:{}] ", modality_key_to_short(t)), Style::default().fg(Color::Green).bg(Color::Rgb(20, 35, 20)))
        }).collect();

        let out_tags: Vec<Span> = m_cfg.output_types.iter().map(|t| {
            Span::styled(format!(" [出:{}] ", modality_key_to_short(t)), Style::default().fg(Color::Cyan).bg(Color::Rgb(20, 30, 45)))
        }).collect();

        let pricing_info = if let Some(p) = current_p {
            let pr = p.get_model_pricing(&m_cfg.name);
            let is_custom_price = m_cfg.input_price.is_some() || m_cfg.cached_price.is_some() || m_cfg.output_price.is_some();
            let eff_mode = p.get_model_billing_mode(&m_cfg.name);
            let is_custom_mode = m_cfg.billing_mode.is_some();
            let mode_str = match eff_mode {
                ponyllm_core::pool::BillingMode::Plan => "Coding Plan (包月订阅/0边际成本)",
                ponyllm_core::pool::BillingMode::Metered => "Metered (按量计费)",
                ponyllm_core::pool::BillingMode::Free => "0元免费 (Free)",
            };
            (
                format!("输入: ${:.2}/1M  |  缓存命中: ${:.3}/1M  |  输出: ${:.2}/1M", pr.input_price, pr.cached_price, pr.output_price),
                if is_custom_price { " [模型独立定价] " } else { " [继承Provider定价] " },
                format!("模式: {}{}", mode_str, if is_custom_mode { " [专属模式]" } else { " [继承Provider模式]" })
            )
        } else {
            ("-".to_string(), "", "模式: metered".to_string())
        };

        let lines = vec![
            Line::from(vec![
                Span::styled("模型标识: ", Style::default().fg(Color::Gray)),
                Span::styled(&m_cfg.name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled("能力梯队: ", Style::default().fg(Color::Gray)),
                Span::styled(format!("{:?} [{}]", m_cfg.tier, m_cfg.tier.shorthand()), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled("默认主模型: ", Style::default().fg(Color::Gray)),
                Span::styled(if is_def { "★ 是 (Primary)" } else { "否" }, if is_def { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) }),
            ]),
            Line::from(vec![
                Span::styled("资费模型: ", Style::default().fg(Color::Gray)),
                Span::styled(pricing_info.0, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(pricing_info.1, Style::default().fg(Color::Green)),
                Span::styled(format!("({})", pricing_info.2), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("上下文窗口: ", Style::default().fg(Color::Gray)),
                Span::styled(format!("{} (Context)", m_cfg.context_window), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled("最大输出: ", Style::default().fg(Color::Gray)),
                Span::styled(format!("{} (Max Output)", m_cfg.max_output), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            ]),
            Line::from({
                let mut v = vec![Span::styled("支持模态: ", Style::default().fg(Color::Gray))];
                v.extend(in_tags);
                v.push(Span::raw("   "));
                v.extend(out_tags);
                v
            }),
            Line::from(vec![
                Span::styled("上游路由: ", Style::default().fg(Color::Gray)),
                Span::styled(format!("{}/v1/chat/completions", base_url.trim_end_matches('/')), Style::default().fg(Color::DarkGray)),
            ]),
        ];

        let spec_card = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(" ── 当前选中模型规格详情 (Model Spec) ── ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: true });
        f.render_widget(spec_card, right_chunks[1]);
    } else {
        let empty_msg = Paragraph::new("暂无选中的模型，按 [a] 快速添加模型参数。")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP).title(" ── 模型规格详情 ── ").border_style(Style::default().fg(Color::DarkGray)));
        f.render_widget(empty_msg, right_chunks[1]);
    }
}

fn render_keys_tab(f: &mut Frame, area: Rect, app: &mut TuiApp) {
    let mut rows: Vec<Row> = Vec::new();
    let sorted_names = app.sorted_provider_names();
    for p_name in &sorted_names {
        if let Some(p) = app.config.providers.get(p_name) {
            for k in &p.keys {
                let masked = ConfigFile::mask_key(&k.api_key);
                rows.push(Row::new(vec![
                    Cell::from(p_name.as_str()),
                    Cell::from(k.id.as_str()),
                    Cell::from(masked),
                    Cell::from(k.priority.to_string()),
                    Cell::from(k.weight.to_string()),
                    Cell::from("🟢 正常就绪"),
                ]));
            }
        }
    }

    if rows.is_empty() {
        let empty = Paragraph::new("暂无 API Key。按 [a] 添加第一个 Key（需先在 [2] 提供商面板添加提供商）。")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(" ── Key 账户池治理 ─────────────────────────────────────────────── ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
        f.render_widget(empty, area);
        return;
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(16),
            Constraint::Percentage(20),
            Constraint::Percentage(28),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
        ],
    )
    .header(
        Row::new(vec!["所属提供商", "Key ID", "API Key (已脱敏)", "优先级", "权重", "状态"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(30, 40, 55))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::TOP)
            .title(" ── Key 账户池治理 ─────────────────────────────────────────────── ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_stateful_widget(table, area, &mut app.key_table_state);
}

fn render_telemetry_tab(f: &mut Frame, area: Rect, app: &mut TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let rows: Vec<Row> = app.flight_frames.iter().map(|frame| {
        let ts = frame.get("timestamp").and_then(|v| v.as_str()).unwrap_or("-");
        let provider = frame.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
        let key_id = frame.get("key_id").and_then(|v| v.as_str()).unwrap_or("-");
        let status = frame.get("status_code").and_then(|v| v.as_u64()).unwrap_or(0);
        let latency = frame.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let err = frame.get("error").and_then(|v| v.as_str()).unwrap_or("-");
        let flow = frame.get("stream_flow");
        let ttft_txt = flow.and_then(|s| s.get("ttft_ms")).and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64))).map(|n| format!("{:.0}", n)).unwrap_or_else(|| "-".to_string());
        let stall_txt = flow.and_then(|s| s.get("stall_count")).and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|n| n as u64))).map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());

        let status_style = if (200..300).contains(&status) {
            Style::default().fg(Color::Green)
        } else if status == 429 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Red)
        };

        Row::new(vec![
            Cell::from(ts),
            Cell::from(provider),
            Cell::from(key_id),
            Cell::from(Span::styled(status.to_string(), status_style)),
            Cell::from(format!("{} ms", latency)),
            Cell::from(ttft_txt),
            Cell::from(stall_txt),
            Cell::from(err),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(18),
            Constraint::Percentage(11),
            Constraint::Percentage(12),
            Constraint::Percentage(8),
            Constraint::Percentage(10),
            Constraint::Percentage(8),
            Constraint::Percentage(7),
            Constraint::Percentage(26),
        ],
    )
    .header(
        Row::new(vec!["时间戳", "提供商", "Key ID", "状态码", "耗时", "TTFT", "stall", "故障原因/录波说明"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(30, 40, 55))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::TOP)
            .title(format!(" ── 黑匣子故障录波帧记录 (最近 {} 条) ────────────────────────── ", app.flight_frames.len()))
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_stateful_widget(table, chunks[0], &mut app.log_table_state);

    // Selected frame detail
    let selected_idx = app.log_table_state.selected().unwrap_or(0);
    let (detail_title, detail_text) = if let Some(frame) = app.flight_frames.get(selected_idx) {
        (
            format!(" ── 录波帧快照详情 (第 {} 条) ────────────────────────────────── ", selected_idx + 1),
            serde_json::to_string_pretty(frame).unwrap_or_else(|_| "无法格式化".to_string()),
        )
    } else {
        (
            " ── 录波帧快照详情 (暂无数据) ────────────────────────────────── ".to_string(),
            "暂无故障录波快照（当前网关运行良好或尚未产生请求）".to_string(),
        )
    };

    let detail = Paragraph::new(detail_text)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title(detail_title)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(detail, chunks[1]);
}

fn safe_centered_rect(min_w: u16, min_h: u16, r: Rect) -> Rect {
    // 响应式宽度：终端较宽时自适应扩展至 75% 宽度，并保证最小 min_w，上限 102 列防过度拉伸
    let responsive_w = (r.width * 75 / 100).max(min_w).min(102);
    let target_w = responsive_w.clamp(20, r.width.saturating_sub(2).max(20));
    let target_h = min_h.clamp(8, r.height.saturating_sub(2).max(8));

    let pad_y = r.height.saturating_sub(target_h) / 2;
    let pad_x = r.width.saturating_sub(target_w) / 2;

    Rect {
        x: r.x + pad_x,
        y: r.y + pad_y,
        width: target_w,
        height: target_h,
    }
}

fn render_modal(f: &mut Frame, area: Rect, app: &TuiApp) {
    match &app.modal {
        Modal::None => {}
        Modal::DeleteProviderConfirm { name } => {
            let modal_area = safe_centered_rect(58, 8, area);
            f.render_widget(Clear, modal_area);

            let text = vec![
                Line::from(Span::styled("⚠️ 安全删除确认", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(format!("确定彻底删除提供商 '{}' 及其所有模型配置？", name)),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [y/Enter] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("确认删除   "),
                    Span::styled(" [n/Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消返回"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Red))
                .title(" 删除提供商 ");
            let p = Paragraph::new(text).block(block).alignment(Alignment::Center);
            f.render_widget(p, modal_area);
        }
        Modal::DeleteModelConfirm { provider_name, model_name } => {
            let modal_area = safe_centered_rect(58, 8, area);
            f.render_widget(Clear, modal_area);

            let text = vec![
                Line::from(Span::styled("⚠️ 删除模型确认", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(format!("确定从 '{}' 中删除模型 '{}' 吗？", provider_name, model_name)),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [y/Enter] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("确认删除   "),
                    Span::styled(" [n/Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消返回"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Red))
                .title(" 删除模型 ");
            let p = Paragraph::new(text).block(block).alignment(Alignment::Center);
            f.render_widget(p, modal_area);
        }
        Modal::AddProvider {
            name,
            base_url,
            default_model,
            strategy_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            active_field,
        } => {
            let modal_area = safe_centered_rect(84, 18, area);
            f.render_widget(Clear, modal_area);

            let strat_options: Vec<Span> = STRATEGIES.iter().enumerate().map(|(i, &s)| {
                let sel = i == *strategy_idx;
                let text = format!(" [{}] {} ", i + 1, s);
                if sel {
                    Span::styled(text, Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled(text, Style::default().fg(Color::DarkGray))
                }
            }).collect();

            let lines = vec![
                Line::from(Span::styled("新建大模型提供商 (Add Provider)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                render_form_field("提供商 ID", name, *active_field == 0),
                render_form_field("Base URL", base_url, *active_field == 1),
                render_form_field("默认模型名", default_model, *active_field == 2),
                Line::from({
                    let mut spans = vec![
                        Span::styled(if *active_field == 3 { "› 调度策略: " } else { "  调度策略: " },
                            if *active_field == 3 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    ];
                    spans.extend(strat_options);
                    spans
                }),
                render_billing_mode_selector(*billing_mode_idx, *active_field == 4, false),
                render_form_field("常规输入单价 ($/1M)", input_price, *active_field == 5),
                render_form_field("缓存命中单价 ($/1M)", cached_price, *active_field == 6),
                render_form_field("输出生成单价 ($/1M)", output_price, *active_field == 7),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [Tab/↓/↑] ", Style::default().fg(Color::Yellow)),
                    Span::raw("切字段  "),
                    Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("保存提交  "),
                    Span::styled(" [Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" 新建提供商 ");
            let p = Paragraph::new(lines).block(block);
            f.render_widget(p, modal_area);
        }
        Modal::EditProvider {
            name,
            base_url,
            default_model,
            strategy_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            active_field,
        } => {
            let modal_area = safe_centered_rect(84, 17, area);
            f.render_widget(Clear, modal_area);

            let strat_options: Vec<Span> = STRATEGIES.iter().enumerate().map(|(i, &s)| {
                let sel = i == *strategy_idx;
                let text = format!(" [{}] {} ", i + 1, s);
                if sel {
                    Span::styled(text, Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled(text, Style::default().fg(Color::DarkGray))
                }
            }).collect();

            let lines = vec![
                Line::from(Span::styled(format!("编辑提供商 [{}]", name), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                render_form_field("Base URL", base_url, *active_field == 0),
                render_form_field("默认模型名", default_model, *active_field == 1),
                Line::from({
                    let mut spans = vec![
                        Span::styled(if *active_field == 2 { "› 调度策略: " } else { "  调度策略: " },
                            if *active_field == 2 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    ];
                    spans.extend(strat_options);
                    spans
                }),
                render_billing_mode_selector(*billing_mode_idx, *active_field == 3, false),
                render_form_field("常规输入单价 ($/1M)", input_price, *active_field == 4),
                render_form_field("缓存命中单价 ($/1M)", cached_price, *active_field == 5),
                render_form_field("输出生成单价 ($/1M)", output_price, *active_field == 6),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [Tab/↓/↑] ", Style::default().fg(Color::Yellow)),
                    Span::raw("切字段  "),
                    Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("保存更改  "),
                    Span::styled(" [Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" 编辑提供商 ");
            let p = Paragraph::new(lines).block(block);
            f.render_widget(p, modal_area);
        }
        Modal::AddModel {
            provider_name,
            model_name,
            tier_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            context_window,
            max_output,
            input_modalities,
            output_modalities,
            set_as_default,
            active_field,
        } => {
            let modal_area = safe_centered_rect(88, 20, area);
            f.render_widget(Clear, modal_area);

            let in_spans = render_modality_checkboxes(input_modalities, *active_field == 8);
            let out_spans = render_modality_checkboxes(output_modalities, *active_field == 9);

            let def_span = if *set_as_default {
                Span::styled(" [x] 设为提供商默认主模型 ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            } else {
                Span::styled(" [ ] 设为提供商默认主模型 (按空格切换) ", Style::default().fg(Color::Gray))
            };

            let lines = vec![
                Line::from(Span::styled(format!("添加模型参数 ── 所属: {}", provider_name), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(""),
                render_form_field("模型标识 (Name)", model_name, *active_field == 0),
                render_tier_selector(*tier_idx, *active_field == 1),
                render_billing_mode_selector(*billing_mode_idx, *active_field == 2, true),
                render_form_field("常规输入单价 ($/1M, 留空继承)", input_price, *active_field == 3),
                render_form_field("缓存命中单价 ($/1M, 留空继承)", cached_price, *active_field == 4),
                render_form_field("输出生成单价 ($/1M, 留空继承)", output_price, *active_field == 5),
                render_form_field("上下文窗口 (如 1M/128K)", context_window, *active_field == 6),
                render_form_field("最大输出限制 (如 32K/64K)", max_output, *active_field == 7),
                Line::from({
                    let mut spans = vec![
                        Span::styled(if *active_field == 8 { "› 输入模态 (1-4): " } else { "  输入模态 (1-4): " },
                            if *active_field == 8 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    ];
                    spans.extend(in_spans);
                    spans
                }),
                Line::from({
                    let mut spans = vec![
                        Span::styled(if *active_field == 9 { "› 输出模态 (1-4): " } else { "  输出模态 (1-4): " },
                            if *active_field == 9 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    ];
                    spans.extend(out_spans);
                    spans
                }),
                Line::from(vec![
                    Span::styled(if *active_field == 10 { "› 默认主模型: " } else { "  默认主模型: " },
                        if *active_field == 10 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    def_span,
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [Tab/↓/↑] ", Style::default().fg(Color::Yellow)),
                    Span::raw("切字段  "),
                    Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("确认添加  "),
                    Span::styled(" [Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" 添加模型配置 ");
            let p = Paragraph::new(lines).block(block);
            f.render_widget(p, modal_area);
        }
        Modal::EditModel {
            provider_name,
            model_name,
            tier_idx,
            billing_mode_idx,
            input_price,
            cached_price,
            output_price,
            context_window,
            max_output,
            input_modalities,
            output_modalities,
            set_as_default,
            active_field,
        } => {
            let modal_area = safe_centered_rect(88, 19, area);
            f.render_widget(Clear, modal_area);

            let in_spans = render_modality_checkboxes(input_modalities, *active_field == 7);
            let out_spans = render_modality_checkboxes(output_modalities, *active_field == 8);

            let def_span = if *set_as_default {
                Span::styled(" [x] 设为提供商默认主模型 ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            } else {
                Span::styled(" [ ] 设为提供商默认主模型 (按空格切换) ", Style::default().fg(Color::Gray))
            };

            let lines = vec![
                Line::from(Span::styled(format!("编辑模型参数: {} ({})", model_name, provider_name), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(""),
                render_tier_selector(*tier_idx, *active_field == 0),
                render_billing_mode_selector(*billing_mode_idx, *active_field == 1, true),
                render_form_field("常规输入单价 ($/1M, 留空继承)", input_price, *active_field == 2),
                render_form_field("缓存命中单价 ($/1M, 留空继承)", cached_price, *active_field == 3),
                render_form_field("输出生成单价 ($/1M, 留空继承)", output_price, *active_field == 4),
                render_form_field("上下文窗口", context_window, *active_field == 5),
                render_form_field("最大输出限制", max_output, *active_field == 6),
                Line::from({
                    let mut spans = vec![
                        Span::styled(if *active_field == 7 { "› 输入模态 (1-4): " } else { "  输入模态 (1-4): " },
                            if *active_field == 7 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    ];
                    spans.extend(in_spans);
                    spans
                }),
                Line::from({
                    let mut spans = vec![
                        Span::styled(if *active_field == 8 { "› 输出模态 (1-4): " } else { "  输出模态 (1-4): " },
                            if *active_field == 8 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    ];
                    spans.extend(out_spans);
                    spans
                }),
                Line::from(vec![
                    Span::styled(if *active_field == 9 { "› 默认主模型: " } else { "  默认主模型: " },
                        if *active_field == 9 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    def_span,
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [Tab/↓/↑] ", Style::default().fg(Color::Yellow)),
                    Span::raw("切字段  "),
                    Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("保存修改  "),
                    Span::styled(" [Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" 编辑模型配置 ");
            let p = Paragraph::new(lines).block(block);
            f.render_widget(p, modal_area);
        }
        Modal::DeleteKeyConfirm { provider, id } => {
            let modal_area = safe_centered_rect(58, 8, area);
            f.render_widget(Clear, modal_area);

            let text = vec![
                Line::from(Span::styled("⚠️ 删除 Key 确认", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(format!("确定从提供商 '{}' 中删除 Key '{}' 吗？", provider, id)),
                Line::from("该操作会立即同步写入配置文件，无法撤销。"),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [y/Enter] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("确认删除   "),
                    Span::styled(" [n/Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消返回"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Red))
                .title(" 删除 Key ");
            let p = Paragraph::new(text).block(block).alignment(Alignment::Center);
            f.render_widget(p, modal_area);
        }
        Modal::AddKey { provider_idx, id, api_key, priority, weight, active_field } => {
            let modal_area = safe_centered_rect(76, 13, area);
            f.render_widget(Clear, modal_area);

            let provider_names = app.sorted_provider_names();
            let current_provider = provider_names
                .get(*provider_idx % provider_names.len().max(1))
                .map(|s| s.as_str())
                .unwrap_or("(无提供商)");

            let provider_span = if *active_field == 0 {
                Span::styled(
                    format!("› 提供商 (←/→ 切换, 共 {} 个): {} ", provider_names.len(), current_provider),
                    Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(
                    format!("  提供商: {}", current_provider),
                    Style::default().fg(Color::Gray),
                )
            };

            let lines = vec![
                Line::from(Span::styled("添加/更新 API Key (Add Key)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(vec![provider_span]),
                render_form_field("Key ID", id, *active_field == 1),
                render_secret_field("API Key", api_key, *active_field == 2),
                render_form_field("优先级 (1最高)", priority, *active_field == 3),
                render_form_field("权重", weight, *active_field == 4),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" [Tab/↓/↑] ", Style::default().fg(Color::Yellow)),
                    Span::raw("切字段  "),
                    Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("确认添加  "),
                    Span::styled(" [Esc] ", Style::default().fg(Color::Gray)),
                    Span::raw("取消"),
                ]),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" 添加 Key ");
            let p = Paragraph::new(lines).block(block);
            f.render_widget(p, modal_area);
        }
    }
}

fn render_form_field<'a>(label: &'a str, value: &'a str, is_active: bool) -> Line<'a> {
    if is_active {
        Line::from(vec![
            Span::styled(format!("› {}: ", label), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(if value.is_empty() { " " } else { value }, Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" █", Style::default().fg(Color::Yellow)),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("  {}: ", label), Style::default().fg(Color::Gray)),
            Span::styled(if value.is_empty() { "(未填写)" } else { value }, Style::default().fg(Color::White)),
        ])
    }
}

/// Render a secret field: plaintext while it is the active/focused field
/// (so the user can verify what they type), masked otherwise to avoid
/// leaking the key onto a shared or recorded terminal.
fn render_secret_field<'a>(label: &'a str, value: &'a str, is_active: bool) -> Line<'a> {
    if is_active {
        Line::from(vec![
            Span::styled(format!("› {}: ", label), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(if value.is_empty() { " ".to_string() } else { value.to_string() }, Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" █", Style::default().fg(Color::Yellow)),
        ])
    } else {
        let shown = if value.is_empty() {
            "(未填写)".to_string()
        } else {
            ConfigFile::mask_key(value)
        };
        Line::from(vec![
            Span::styled(format!("  {}: ", label), Style::default().fg(Color::Gray)),
            Span::styled(shown, Style::default().fg(Color::White)),
        ])
    }
}

fn render_modality_checkboxes<'a>(modalities: &[bool; 4], is_active: bool) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    for (i, &on) in modalities.iter().enumerate() {
        let label = MODALITIES[i];
        let tag = if on {
            format!(" [x] {}:{} ", i + 1, label)
        } else {
            format!(" [ ] {}:{} ", i + 1, label)
        };

        let style = if on {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        spans.push(Span::styled(tag, style));
    }
    spans
}

fn render_tier_selector(tier_idx: usize, is_active: bool) -> Line<'static> {
    let prefix = if is_active { "› 能力梯队 (按1-3/空格切换): " } else { "  能力梯队 (按1-3/空格切换): " };
    let prefix_style = if is_active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };

    let make_item = |idx: usize, label: &'static str| {
        let is_sel = idx == tier_idx;
        if is_sel {
            Span::styled(
                format!(" [•] {} ", label),
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" [ ] {} ", label), Style::default().fg(Color::DarkGray))
        }
    };

    Line::from(vec![
        Span::styled(prefix, prefix_style),
        make_item(0, "1:Flagship"),
        Span::raw(" "),
        make_item(1, "2:Standard"),
        Span::raw(" "),
        make_item(2, "3:Light"),
    ])
}

fn billing_mode_to_provider_idx(mode: BillingMode) -> usize {
    match mode {
        BillingMode::Metered => 0,
        BillingMode::Plan => 1,
        BillingMode::Free => 2,
    }
}

fn idx_to_provider_billing_mode(idx: usize) -> BillingMode {
    match idx {
        1 => BillingMode::Plan,
        2 => BillingMode::Free,
        _ => BillingMode::Metered,
    }
}

fn model_billing_mode_to_idx(mode: Option<BillingMode>) -> usize {
    match mode {
        None => 0,
        Some(BillingMode::Metered) => 1,
        Some(BillingMode::Plan) => 2,
        Some(BillingMode::Free) => 3,
    }
}

fn idx_to_model_billing_mode(idx: usize) -> Option<BillingMode> {
    match idx {
        1 => Some(BillingMode::Metered),
        2 => Some(BillingMode::Plan),
        3 => Some(BillingMode::Free),
        _ => None,
    }
}

fn render_billing_mode_selector(mode_idx: usize, is_active: bool, is_model_level: bool) -> Line<'static> {
    let prefix = if is_active { "› 计费模式/是否订阅 (按1-4/空格切换): " } else { "  计费模式/是否订阅 (按1-4/空格切换): " };
    let prefix_style = if is_active {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };

    let make_item = |idx: usize, label: &'static str| {
        let is_sel = idx == mode_idx;
        if is_sel {
            Span::styled(
                format!(" [•] {} ", label),
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" [ ] {} ", label), Style::default().fg(Color::DarkGray))
        }
    };

    if is_model_level {
        Line::from(vec![
            Span::styled(prefix, prefix_style),
            make_item(0, "1:继承"),
            Span::raw(" "),
            make_item(1, "2:按量"),
            Span::raw(" "),
            make_item(2, "3:Coding Plan订阅"),
            Span::raw(" "),
            make_item(3, "4:免费"),
        ])
    } else {
        Line::from(vec![
            Span::styled(prefix, prefix_style),
            make_item(0, "1:按量(Metered)"),
            Span::raw(" "),
            make_item(1, "2:Coding Plan(包月订阅)"),
            Span::raw(" "),
            make_item(2, "3:0元免费"),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;

    fn default_app() -> (TuiApp, tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("ponyllm.toml").to_string_lossy().to_string();
        let mut cfg = ConfigFile::default();
        cfg.add_provider("bai", "https://api.bai.com", "bai-v4", "round_robin");
        cfg.add_provider("openai", "https://api.openai.com", "gpt-4o", "priority");
        let app = TuiApp::new(cfg, cfg_path.clone(), "http://127.0.0.1:8080".to_string());
        (app, tmp, cfg_path)
    }

    #[test]
    fn test_key_tab_add_via_modal() {
        let (mut app, _tmp, _cfg_path) = default_app();
        app.active_tab = 2;

        // Press [a] on the Key tab -> opens the AddKey modal
        handle_key_event(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        // No key selected yet -> provider_idx defaults to the first provider (sorted: bai)
        match &app.modal {
            Modal::AddKey { provider_idx, .. } => assert_eq!(*provider_idx, 0),
            _ => panic!("expected AddKey modal"),
        }

        // Fill fields and submit
        app.modal = Modal::AddKey {
            provider_idx: 0, // "bai" (sorted)
            id: "k1".to_string(),
            api_key: "sk-bai-test-123".to_string(),
            priority: "2".to_string(),
            weight: "8".to_string(),
            active_field: 1,
        };
        handle_modal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        // On success the modal is dismissed and the key is persisted to config
        assert!(matches!(app.modal, Modal::None));
        let p = &app.config.providers["bai"];
        assert_eq!(p.keys.len(), 1);
        assert_eq!(p.keys[0].id, "k1");
        assert_eq!(p.keys[0].priority, 2);
        assert_eq!(p.keys[0].weight, 8);

        // selected_key() resolves the selected row back to (provider, key)
        app.key_table_state.select(Some(0));
        let (owner, key) = app.selected_key().unwrap();
        assert_eq!(owner, "bai");
        assert_eq!(key.id, "k1");
    }

    #[test]
    fn test_key_tab_add_rejects_invalid_priority() {
        let (mut app, _tmp, _cfg_path) = default_app();
        app.active_tab = 2;

        // A non-numeric priority must NOT be silently coerced to 1
        app.modal = Modal::AddKey {
            provider_idx: 0,
            id: "k1".to_string(),
            api_key: "sk-abc".to_string(),
            priority: "not-a-number".to_string(),
            weight: "10".to_string(),
            active_field: 1,
        };
        handle_modal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        // The modal stays open and the key is NOT persisted
        assert!(matches!(app.modal, Modal::AddKey { .. }));
        assert_eq!(app.config.providers["bai"].keys.len(), 0);
    }

    #[test]
    fn test_key_tab_delete_via_modal() {
        let (mut app, _tmp, _cfg_path) = default_app();
        app.config.add_key("bai", "k1", "sk-abc", 1, 10).unwrap();
        app.active_tab = 2;
        app.key_table_state.select(Some(0));

        // Press [d] on the Key tab -> opens DeleteKeyConfirm for the selected row
        handle_key_event(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(matches!(app.modal, Modal::DeleteKeyConfirm { .. }));

        handle_modal_key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(matches!(app.modal, Modal::None));
        assert_eq!(app.config.providers["bai"].keys.len(), 0);
    }

    #[test]
    fn test_key_tab_add_requires_provider() {
        let cfg = ConfigFile::default(); // no providers configured
        let tmp = tempfile::tempdir().unwrap();
        let mut app = TuiApp::new(
            cfg,
            tmp.path().join("c.toml").to_string_lossy().to_string(),
            "http://127.0.0.1:8080".to_string(),
        );
        app.active_tab = 2;

        handle_key_event(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        // No provider => must NOT open the AddKey modal
        assert!(matches!(app.modal, Modal::None));
    }

    #[test]
    fn test_key_tab_delete_without_selection() {
        let (mut app, _tmp, _cfg_path) = default_app();
        app.active_tab = 2;
        app.key_table_state.select(None);

        handle_key_event(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(matches!(app.modal, Modal::None));
    }

    #[test]
    fn test_tab2_add_model_modal_tier_billing_mode_and_pricing_submit() {
        let (mut app, _tmp, _cfg_path) = default_app();
        app.active_tab = 1;
        app.tab2_focus = Tab2Focus::Models;
        app.provider_table_state.select(Some(0)); // "bai"

        // 唤起 AddModel 模态框
        handle_key_event(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        match &app.modal {
            Modal::AddModel { tier_idx, billing_mode_idx, .. } => {
                assert_eq!(*tier_idx, 1); // 默认 Standard
                assert_eq!(*billing_mode_idx, 0); // 默认继承提供商
            }
            _ => panic!("expected AddModel modal"),
        }

        // 模拟用户输入：
        // 0: model_name, 1: tier, 2: billing_mode, 3: input_price, 4: cached_price, 5: output_price
        app.modal = Modal::AddModel {
            provider_name: "bai".to_string(),
            model_name: "bai-coder-pro".to_string(),
            tier_idx: 0, // Flagship (按数字 '1' 或空格)
            billing_mode_idx: 2, // Coding Plan (按数字 '3')
            input_price: "1.25".to_string(),
            cached_price: "0.15".to_string(),
            output_price: "2.50".to_string(),
            context_window: "256K".to_string(),
            max_output: "16K".to_string(),
            input_modalities: [true, false, false, false],
            output_modalities: [true, false, false, false],
            set_as_default: true,
            active_field: 0,
        };

        handle_modal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        // 弹窗关闭并成功持久化
        assert!(matches!(app.modal, Modal::None));
        let p = &app.config.providers["bai"];
        assert_eq!(p.default_model, "bai-coder-pro");
        let m = p.get_model_config("bai-coder-pro");
        assert_eq!(m.tier, ponyllm_core::pool::ModelTier::Flagship);
        assert_eq!(m.billing_mode, Some(ponyllm_core::pool::BillingMode::Plan));
        assert_eq!(m.input_price, Some(1.25));
        assert_eq!(m.cached_price, Some(0.15));
        assert_eq!(m.output_price, Some(2.50));
        assert_eq!(p.get_model_billing_mode("bai-coder-pro"), ponyllm_core::pool::BillingMode::Plan);
    }

    #[test]
    fn test_tab2_add_provider_modal_billing_and_pricing_submit() {
        let (mut app, _tmp, _cfg_path) = default_app();
        app.active_tab = 1;
        app.tab2_focus = Tab2Focus::Providers;

        // 模拟 AddProvider 并指定 Coding Plan 与独立基准资费
        app.modal = Modal::AddProvider {
            name: "siliconflow".to_string(),
            base_url: "https://api.siliconflow.cn".to_string(),
            default_model: "deepseek-v3".to_string(),
            strategy_idx: 0,
            billing_mode_idx: 1, // Coding Plan
            input_price: "0.20".to_string(),
            cached_price: "0.05".to_string(),
            output_price: "0.40".to_string(),
            active_field: 0,
        };

        handle_modal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.modal, Modal::None));
        let p = &app.config.providers["siliconflow"];
        assert_eq!(p.billing_mode, ponyllm_core::pool::BillingMode::Plan);
        assert_eq!(p.input_price, 0.20);
        assert_eq!(p.cached_price, 0.05);
        assert_eq!(p.output_price, 0.40);
    }

    #[test]
    fn test_modalities_short_labels_and_compact_rendering() {
        assert_eq!(modality_key_to_short("text"), "文(Txt)");
        assert_eq!(modality_key_to_short("image"), "图(Img)");
        assert_eq!(modality_key_to_short("video"), "视(Vid)");
        assert_eq!(modality_key_to_short("audio"), "音(Aud)");

        let mods = [true, false, true, false];
        let spans = render_modality_checkboxes(&mods, true);
        assert_eq!(spans.len(), 4);
        assert!(spans[0].content.contains("[x] 1:文 (Txt)"));
        assert!(spans[1].content.contains("[ ] 2:图 (Img)"));
        assert!(spans[2].content.contains("[x] 3:视 (Vid)"));
        assert!(spans[3].content.contains("[ ] 4:音 (Aud)"));

        // 计算所有模态选项拼接的总字符宽度，确保在 65 列以内，彻底防止溢出弹窗
        let total_chars: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(total_chars <= 65, "Total chars {} exceeds compact budget 65", total_chars);
    }

    #[test]
    fn test_safe_centered_rect_responsive_widening() {
        // 1. 标准 80 列屏幕
        let r80 = Rect { x: 0, y: 0, width: 80, height: 24 };
        let box80 = safe_centered_rect(88, 20, r80);
        // 80列屏幕保留至少左右边距2列，width 78
        assert_eq!(box80.width, 78);
        assert_eq!(box80.height, 20);

        // 2. 宽屏 120 列屏幕：按 75% 响应式扩展或至少 min_w(88)
        let r120 = Rect { x: 0, y: 0, width: 120, height: 30 };
        let box120 = safe_centered_rect(88, 20, r120);
        // 120 * 75% = 90 列
        assert_eq!(box120.width, 90);

        // 3. 超宽屏 160 列屏幕：上限 clamp 在 102 列防过度拉伸
        let r160 = Rect { x: 0, y: 0, width: 160, height: 40 };
        let box160 = safe_centered_rect(88, 20, r160);
        assert_eq!(box160.width, 102);
    }
}
