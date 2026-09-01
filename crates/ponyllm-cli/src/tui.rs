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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState, Tabs, Wrap,
    },
    Frame, Terminal,
};
use serde_json::Value;
use crate::config::ConfigFile;

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

pub struct TuiApp {
    pub active_tab: usize,
    pub config: ConfigFile,
    pub config_path: String,
    pub gateway_url: String,
    pub is_online: bool,
    pub metrics: Option<Value>,
    pub flight_frames: Vec<Value>,
    pub provider_table_state: TableState,
    pub key_table_state: TableState,
    pub log_table_state: TableState,
    pub should_quit: bool,
    pub status_message: String,
}

impl TuiApp {
    pub fn new(config: ConfigFile, config_path: String, gateway_url: String) -> Self {
        let mut provider_state = TableState::default();
        provider_state.select(Some(0));
        let mut key_state = TableState::default();
        key_state.select(Some(0));
        let mut log_state = TableState::default();
        log_state.select(Some(0));

        Self {
            active_tab: 0,
            config,
            config_path,
            gateway_url,
            is_online: false,
            metrics: None,
            flight_frames: Vec::new(),
            provider_table_state: provider_state,
            key_table_state: key_state,
            log_table_state: log_state,
            should_quit: false,
            status_message: "就绪。按 [Tab] 切换面板，[r] 刷新指标，[q] 退出。".to_string(),
        }
    }

    pub async fn poll_gateway(&mut self) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1500))
            .build()
            .unwrap_or_default();

        let base = self.gateway_url.trim_end_matches('/');
        let health_url = format!("{}/health", base);
        let metrics_url = format!("{}/v1/telemetry/metrics", base);
        let rec_url = format!("{}/v1/telemetry/recorder", base);

        if let Ok(resp) = client.get(&health_url).send().await {
            if resp.status().is_success() {
                self.is_online = true;
                if let Ok(m) = client.get(&metrics_url).send().await {
                    if let Ok(val) = m.json::<Value>().await {
                        self.metrics = Some(val);
                    }
                }
                if let Ok(r) = client.get(&rec_url).send().await {
                    if let Ok(val) = r.json::<Value>().await {
                        if let Some(arr) = val.as_array() {
                            self.flight_frames = arr.clone();
                        }
                    }
                }
                return;
            }
        }
        self.is_online = false;
    }
}

pub async fn run_tui(config: ConfigFile, config_path: String, gateway_url: String) -> Result<(), Box<dyn std::error::Error>> {
    let mut guard = TerminalGuard::new()?;
    let mut app = TuiApp::new(config, config_path, gateway_url);
    app.poll_gateway().await;

    let mut reader = crossterm::event::EventStream::new();
    let mut poll_interval = tokio::time::interval(Duration::from_millis(1000));

    loop {
        guard.terminal.draw(|f| ui(f, &mut app))?;

        tokio::select! {
            _ = poll_interval.tick() => {
                app.poll_gateway().await;
            }
            maybe_event = reader.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind == KeyEventKind::Press {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                            app.should_quit = true;
                        } else {
                            match key.code {
                                KeyCode::Char('q') | KeyCode::Esc => {
                                    app.should_quit = true;
                                }
                                KeyCode::Tab => {
                                    app.active_tab = (app.active_tab + 1) % 4;
                                }
                                KeyCode::BackTab => {
                                    app.active_tab = if app.active_tab == 0 { 3 } else { app.active_tab - 1 };
                                }
                                KeyCode::Char('1') => app.active_tab = 0,
                                KeyCode::Char('2') => app.active_tab = 1,
                                KeyCode::Char('3') => app.active_tab = 2,
                                KeyCode::Char('4') => app.active_tab = 3,
                                KeyCode::Char('r') => {
                                    app.poll_gateway().await;
                                    app.status_message = "已手动拉取最新遥测数据。".to_string();
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    match app.active_tab {
                                        1 => scroll_table(&mut app.provider_table_state, app.config.providers.len(), 1),
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
                                        1 => scroll_table(&mut app.provider_table_state, app.config.providers.len(), -1),
                                        2 => {
                                            let total_keys = app.config.providers.values().map(|p| p.keys.len()).sum();
                                            scroll_table(&mut app.key_table_state, total_keys, -1);
                                        }
                                        3 => scroll_table(&mut app.log_table_state, app.flight_frames.len(), -1),
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
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
        .block(Block::default().borders(Borders::ALL).title(" 尺寸警告 "));
        f.render_widget(warn_msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header & Tabs
            Constraint::Min(0),    // Main Content Area
            Constraint::Length(3), // Footer / Status Bar
        ])
        .split(area);

    // 1. Header Tabs
    let tab_titles = vec![
        "1 📊 实时监控大盘",
        "2 🏢 提供商 & 模型",
        "3 🔑 Key 账户池治理",
        "4 📼 黑匣子故障录波",
    ];
    let tabs = Tabs::new(tab_titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" ponyllm 统一网关控制台 ")
                .border_type(BorderType::Rounded),
        )
        .select(app.active_tab)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    // 2. Tab Contents
    match app.active_tab {
        0 => render_overview_tab(f, chunks[1], app),
        1 => render_providers_tab(f, chunks[1], app),
        2 => render_keys_tab(f, chunks[1], app),
        3 => render_telemetry_tab(f, chunks[1], app),
        _ => {}
    }

    // 3. Footer / Help Bar
    let status_style = if app.is_online {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let online_label = if app.is_online {
        "● 网关在线 (ONLINE)"
    } else {
        "○ 网关未连通 (OFFLINE - 请先执行 ponyllm serve)"
    };

    let footer = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(format!(" {} ", online_label), status_style.add_modifier(Modifier::BOLD)),
            Span::raw(" | "),
            Span::styled(&app.status_message, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(" [Tab] ", Style::default().fg(Color::Yellow)),
            Span::raw("切页  "),
            Span::styled(" [↑/↓/j/k] ", Style::default().fg(Color::Yellow)),
            Span::raw("上下移动  "),
            Span::styled(" [r] ", Style::default().fg(Color::Yellow)),
            Span::raw("手动刷新  "),
            Span::styled(" [q/Esc/Ctrl+C] ", Style::default().fg(Color::Yellow)),
            Span::raw("退出控制台"),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded));
    f.render_widget(footer, chunks[2]);
}

fn render_overview_tab(f: &mut Frame, area: Rect, app: &TuiApp) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
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
        .and_then(|m| m.get("total_success").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let total_failover = app.metrics.as_ref()
        .and_then(|m| m.get("total_failover").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let total_errors = app.metrics.as_ref()
        .and_then(|m| m.get("total_errors").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    let card1 = Paragraph::new(vec![
        Line::from("网关监听地址"),
        Line::from(Span::styled(&app.config.gateway.bind, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(format!("服务状态: {}", if app.is_online { "🟢 UP" } else { "🔴 DOWN" })),
    ])
    .block(Block::default().borders(Borders::ALL).title(" 基础信息 ").border_type(BorderType::Rounded));
    f.render_widget(card1, top_row[0]);

    let card2 = Paragraph::new(vec![
        Line::from("总请求流量"),
        Line::from(Span::styled(format!("{} 次", total_reqs), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))),
        Line::from(format!("成功调用: {} 次", total_success)),
    ])
    .block(Block::default().borders(Borders::ALL).title(" 请求统计 ").border_type(BorderType::Rounded));
    f.render_widget(card2, top_row[1]);

    let card3 = Paragraph::new(vec![
        Line::from("429 / 故障自动倒换"),
        Line::from(Span::styled(format!("{} 次", total_failover), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from("无感毫秒级熔断倒换"),
    ])
    .block(Block::default().borders(Borders::ALL).title(" 容灾倒换 ").border_type(BorderType::Rounded));
    f.render_widget(card3, top_row[2]);

    let card4 = Paragraph::new(vec![
        Line::from("不可恢复异常 (5xx)"),
        Line::from(Span::styled(format!("{} 次", total_errors), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
        Line::from("全链路故障录波保障"),
    ])
    .block(Block::default().borders(Borders::ALL).title(" 异常追踪 ").border_type(BorderType::Rounded));
    f.render_widget(card4, top_row[3]);

    // Bottom Half
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[1]);

    // Provider summary
    let prov_rows: Vec<Row> = app.config.providers.iter().map(|(name, p)| {
        Row::new(vec![
            Cell::from(name.as_str()),
            Cell::from(p.default_model.as_str()),
            Cell::from(p.strategy.as_str()),
            Cell::from(p.keys.len().to_string()),
        ])
    }).collect();

    let prov_table = Table::new(
        prov_rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(35),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
        ],
    )
    .header(Row::new(vec!["提供商", "默认模型", "调度策略", "Key 数量"]).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
    .block(Block::default().borders(Borders::ALL).title(" 已挂载模型提供商 ").border_type(BorderType::Rounded));
    f.render_widget(prov_table, bottom_chunks[0]);

    // Quickstart & Features
    let info = Paragraph::new(vec![
        Line::from(Span::styled("双向全协议转译引擎:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from("  • OpenAI Chat Completions ⇄ Anthropic Messages ⇄ Responses API"),
        Line::from("  • 完整支持 DeepSeek Reasoning 思考链无损传递"),
        Line::from(""),
        Line::from(Span::styled("多 Key 账户池与熔断机制:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from("  • 429 频率超限自动冷却 (Cooldown) + 402/403 配额耗尽剔除"),
        Line::from("  • TTFT 首字节喷出前全透明自动故障倒换 (Failover)"),
        Line::from(""),
        Line::from(Span::styled("快捷 CLI 命令:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from("  • ponyllm provider add / list / remove"),
        Line::from("  • ponyllm key add / list / remove / test"),
    ])
    .block(Block::default().borders(Borders::ALL).title(" 网关特性与使用指南 ").border_type(BorderType::Rounded))
    .wrap(Wrap { trim: true });
    f.render_widget(info, bottom_chunks[1]);
}

fn render_providers_tab(f: &mut Frame, area: Rect, app: &mut TuiApp) {
    let rows: Vec<Row> = app.config.providers.iter().map(|(name, p)| {
        Row::new(vec![
            Cell::from(name.as_str()),
            Cell::from(p.base_url.as_str()),
            Cell::from(p.default_model.as_str()),
            Cell::from(p.strategy.as_str()),
            Cell::from(p.keys.len().to_string()),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(18),
            Constraint::Percentage(32),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(10),
        ],
    )
    .header(
        Row::new(vec!["提供商 (Provider)", "Base URL", "默认模型 (Model)", "调度策略", "Key 数量"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 提供商列表 (共 {} 个) - 可使用 'ponyllm provider add' 增量扩展 ", app.config.providers.len()))
            .border_type(BorderType::Rounded),
    );

    f.render_stateful_widget(table, area, &mut app.provider_table_state);
}

fn render_keys_tab(f: &mut Frame, area: Rect, app: &mut TuiApp) {
    let mut rows: Vec<Row> = Vec::new();
    for (p_name, p) in &app.config.providers {
        for k in &p.keys {
            let masked = ConfigFile::mask_key(&k.api_key);
            rows.push(Row::new(vec![
                Cell::from(p_name.as_str()),
                Cell::from(k.id.as_str()),
                Cell::from(masked),
                Cell::from(k.priority.to_string()),
                Cell::from(k.weight.to_string()),
                Cell::from("🟢 就绪 (Active)"),
            ]));
        }
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(16),
        ],
    )
    .header(
        Row::new(vec!["所属提供商", "Key ID", "API Key (已脱敏)", "优先级", "权重", "健康状态"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Key 账户池管理 - 支持 'ponyllm key add / remove / test' ")
            .border_type(BorderType::Rounded),
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

        let status_style = if status >= 200 && status < 300 {
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
            Cell::from(err),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(22),
            Constraint::Percentage(13),
            Constraint::Percentage(15),
            Constraint::Percentage(10),
            Constraint::Percentage(12),
            Constraint::Percentage(28),
        ],
    )
    .header(
        Row::new(vec!["时间戳", "提供商", "Key ID", "状态码", "耗时", "故障原因/录波说明"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 黑匣子故障录波帧记录 (最近 {} 条) ", app.flight_frames.len()))
            .border_type(BorderType::Rounded),
    );

    f.render_stateful_widget(table, chunks[0], &mut app.log_table_state);

    // Selected frame detail
    let selected_idx = app.log_table_state.selected().unwrap_or(0);
    let detail_text = if let Some(frame) = app.flight_frames.get(selected_idx) {
        serde_json::to_string_pretty(frame).unwrap_or_else(|_| "无法格式化".to_string())
    } else {
        "暂无故障录波快照（当前网关运行良好或尚未产生请求）".to_string()
    };

    let detail = Paragraph::new(detail_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" 录波帧快照详情 (第 {} 条) ", selected_idx + 1))
                .border_type(BorderType::Rounded),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(detail, chunks[1]);
}
