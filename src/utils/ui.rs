// 美观的 CLI 输出模块

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::proxy::metrics::Metrics;

// ─────────────────────────────────────────────────────────────
// ANSI 颜色代码
// ─────────────────────────────────────────────────────────────

const RESET: &str = "\x1B[0m";
const BOLD: &str = "\x1B[1m";
const DIM: &str = "\x1B[2m";

const GREEN: &str = "\x1B[32m";
const RED: &str = "\x1B[31m";
const YELLOW: &str = "\x1B[33m";

// 粉色系
const PINK_PRIMARY: &str = "\x1B[38;5;219m";
const PINK_SECONDARY: &str = "\x1B[38;5;213m";
const PINK_ACCENT: &str = "\x1B[38;5;206m";
const PINK_GLOW: &str = "\x1B[38;5;177m";
const PINK_SOFT: &str = "\x1B[38;5;225m";

// 晨光系
const MORNING_PRIMARY: &str = "\x1B[38;5;220m";
const MORNING_SECONDARY: &str = "\x1B[38;5;209m";
const MORNING_ACCENT: &str = "\x1B[38;5;214m";
const MORNING_GLOW: &str = "\x1B[38;5;178m";
const MORNING_SOFT: &str = "\x1B[38;5;222m";

// 薄荷系
const NOON_PRIMARY: &str = "\x1B[38;5;123m";
const NOON_SECONDARY: &str = "\x1B[38;5;122m";
const NOON_ACCENT: &str = "\x1B[38;5;117m";
const NOON_GLOW: &str = "\x1B[38;5;154m";
const NOON_SOFT: &str = "\x1B[38;5;194m";

// 深夜系（代码喵）- 深蓝/青色系
const NIGHT_PRIMARY: &str = "\x1b[38;5;39m";      // 深蓝
const NIGHT_SECONDARY: &str = "\x1b[38;5;45m";    // 青色
const NIGHT_ACCENT: &str = "\x1b[38;5;51m";       // 亮青
const NIGHT_GLOW: &str = "\x1b[38;5;81m";         // 浅蓝
const NIGHT_SOFT: &str = "\x1b[38;5;117m";        // 淡青

// ─────────────────────────────────────────────────────────────
// 主题系统
// ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum TimeTheme {
    Morning,
    Noon,
    Night,
}

fn time_theme_now() -> TimeTheme {
    let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
    match hour {
        6..=11 => TimeTheme::Morning,
        12..=17 => TimeTheme::Noon,
        _ => TimeTheme::Night,
    }
}

pub struct ThemeColors {
    primary: &'static str,
    secondary: &'static str,
    accent: &'static str,
    glow: &'static str,
    soft: &'static str,
}

impl ThemeColors {
    pub fn pink() -> Self {
        ThemeColors {
            primary: PINK_PRIMARY,
            secondary: PINK_SECONDARY,
            accent: PINK_ACCENT,
            glow: PINK_GLOW,
            soft: PINK_SOFT,
        }
    }

    pub fn morning() -> Self {
        ThemeColors {
            primary: MORNING_PRIMARY,
            secondary: MORNING_SECONDARY,
            accent: MORNING_ACCENT,
            glow: MORNING_GLOW,
            soft: MORNING_SOFT,
        }
    }

    pub fn noon() -> Self {
        ThemeColors {
            primary: NOON_PRIMARY,
            secondary: NOON_SECONDARY,
            accent: NOON_ACCENT,
            glow: NOON_GLOW,
            soft: NOON_SOFT,
        }
    }

    pub fn night() -> Self {
        ThemeColors {
            primary: NIGHT_PRIMARY,
            secondary: NIGHT_SECONDARY,
            accent: NIGHT_ACCENT,
            glow: NIGHT_GLOW,
            soft: NIGHT_SOFT,
        }
    }

    pub fn from_config(theme_name: &str) -> Self {
        match theme_name {
            "pink" => Self::pink(),
            "morning" => Self::morning(),
            "noon" => Self::noon(),
            "night" => Self::night(),
            "auto" => match time_theme_now() {
                TimeTheme::Morning => Self::morning(),
                TimeTheme::Noon => Self::noon(),
                TimeTheme::Night => Self::night(),
            },
            _ => Self::pink(),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 颜色辅助函数
// ─────────────────────────────────────────────────────────────

fn colorize(text: &str, color: &str) -> String {
    format!("{}{}{}", color, text, RESET)
}

fn bold(text: &str) -> String {
    format!("{}{}{}", BOLD, text, RESET)
}

fn dim(text: &str) -> String {
    format!("{}{}{}", DIM, text, RESET)
}

// ─────────────────────────────────────────────────────────────
// 颜文字（Kaomoji）
// ─────────────────────────────────────────────────────────────

mod kaomoji {
    pub const CAT: &[&str] = &[
        "(=^･ω･^=)", "(=^･ｪ･^=)", "(ΦωΦ)", "(^w^)",
    ];
    pub const WORKING: &[&str] = &[
        "(･_･)", "(｡_｡)", "(・_・)", "(°_°)",
    ];
    pub const SUCCESS: &[&str] = &[
        "(^_^)b", "(^_^)v", "(≧▽≦)", "ヽ(^_^ )",
    ];
    pub const SLEEPY: &[&str] = &[
        "(-_-)zzZ", "(u_u)", "(－_－) zzZ",
    ];
    pub const LOVE: &[&str] = &[
        "(♥_♥)", "(♡▽♡)", "(´,,•ω•,,)",
    ];
}

fn pick_random(items: &[&str]) -> &'static str {
    let idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as usize)
        .unwrap_or(0)) % items.len();
    unsafe { std::mem::transmute(items[idx]) }
}

// ─────────────────────────────────────────────────────────────
// Unicode 框线字符
// ─────────────────────────────────────────────────────────────

mod box_drawing {
    pub const HORIZONTAL: &str = "─";
    pub const VERTICAL: &str = "│";
    pub const TOP_LEFT: &str = "╭";
    pub const BOTTOM_LEFT: &str = "╰";
    pub const LEFT_TEE: &str = "├";
}

// 渐变分隔线（带动画帧）
fn gradient_divider(theme: &ThemeColors, width: usize, frame: usize) -> String {
    let gradient = [theme.primary, theme.secondary, theme.accent, theme.glow, theme.soft];
    let chars = ["━", "╾", "─", "╼"];

    let mut line = String::new();
    for i in 0..width {
        let color_idx = (i * gradient.len() / width) % gradient.len();
        let char_idx = (i + frame) % chars.len();
        line.push_str(gradient[color_idx]);
        line.push_str(chars[char_idx]);
    }
    line.push_str(RESET);
    line
}

// 静态渐变分隔线
fn divider(theme: &ThemeColors, width: usize) -> String {
    gradient_divider(theme, width, 0)
}

// ─────────────────────────────────────────────────────────────
// 启动动画
// ─────────────────────────────────────────────────────────────

pub struct StartupUI {
    theme: ThemeColors,
    current_step: AtomicUsize,
    total_steps: usize,
}

impl StartupUI {
    pub fn new(theme_name: &str) -> Self {
        Self {
            theme: ThemeColors::from_config(theme_name),
            current_step: AtomicUsize::new(0),
            total_steps: 4,  // DNS -> CA -> 代理 -> 服务
        }
    }

    pub fn print_header(&self) {
        let width = 50;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);
        eprintln!("  {}{}  {} 启动中... {}",
            self.theme.primary, box_drawing::VERTICAL,
            bold("Hakimi Hub"),
            pick_random(kaomoji::WORKING));
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::LEFT_TEE, line);
    }

    pub fn step(&self, icon: &str, message: &str) {
        let step = self.current_step.fetch_add(1, Ordering::SeqCst) + 1;
        let progress = format!("[{}/{}]", step, self.total_steps);

        eprintln!("  {}{} {} {} {}",
            self.theme.primary, box_drawing::VERTICAL,
            dim(&progress),
            colorize(icon, self.theme.accent),
            message);

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    pub fn step_dns(&self) {
        self.step("[OK]", "初始化 DNS 解析器");
    }

    pub fn step_ca(&self, loaded: bool) {
        let msg = if loaded { "加载 CA 证书" } else { "生成 CA 证书" };
        self.step("[OK]", msg);
    }

    pub fn step_proxy(&self) {
        self.step("[OK]", "设置系统代理");
    }

    pub fn step_server(&self) {
        self.step("[OK]", "启动代理服务");
    }

    pub fn finish(&self, bind: &str, port: u16, mitm: bool) {
        let width = 52;
        let line = divider(&self.theme, width);

        eprintln!("  {}{}{}", self.theme.primary, box_drawing::LEFT_TEE, line);

        let face = pick_random(kaomoji::SUCCESS);

        eprintln!("  {}{}  {} {} 启动成功! {}",
            self.theme.primary, box_drawing::VERTICAL,
            bold("Hakimi Hub"), face, RESET);

        eprintln!("  {}{}  代理地址: {}",
            self.theme.primary, box_drawing::VERTICAL,
            colorize(&format!("http://{}:{}", bind, port), self.theme.accent));

        let mitm_status = if mitm {
            colorize("已启用", self.theme.accent)
        } else {
            dim("已禁用")
        };
        eprintln!("  {}{}  MITM 拦截: {}",
            self.theme.primary, box_drawing::VERTICAL,
            mitm_status);

        if mitm {
            eprintln!("  {}{}  PAC 地址: {}",
                self.theme.primary, box_drawing::VERTICAL,
                colorize(&format!("http://{}:{}/pac", bind, port), self.theme.secondary));
        }

        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);

        // 停留 3 秒让用户看清启动信息
        std::thread::sleep(std::time::Duration::from_millis(3000));

        // 计算启动过程的行数：
        // 启动步骤: 4 行（header + 4 steps）
        // 启动成功框: 顶部线 + 标题 + 地址 + MITM + PAC? + 底部线
        let header_lines = 3;  // print_header 打印的行数
        let step_lines = 4;    // 4 个步骤
        let success_lines = if mitm { 6 } else { 5 };

        // 总共需要清理的行数
        let total_clear_lines = header_lines + step_lines + success_lines;

        // 渐隐清理（每行 80ms）
        for _ in 0..total_clear_lines {
            eprint!("\x1B[A\x1B[K");  // 上移一行并清除
            std::thread::sleep(std::time::Duration::from_millis(80));
        }

        // 为运行面板预留空间（10 行）
        for _ in 0..10 {
            eprintln!();
        }
        eprint!("\x1B[10A");
    }

    pub fn error(&self, message: &str) {
        let width = 50;
        let line = box_drawing::HORIZONTAL.repeat(width);

        eprintln!("  {}{}{}", RED, box_drawing::LEFT_TEE, line);
        eprintln!("  {}{}  {} 启动失败 {}",
            RED, box_drawing::VERTICAL,
            bold("Hakimi Hub"),
            pick_random(kaomoji::SLEEPY));
        eprintln!("  {}{}  错误: {}", RED, box_drawing::VERTICAL, message);
        eprintln!("  {}{}{}", RED, box_drawing::BOTTOM_LEFT, line);
        eprintln!("{}", RESET);
    }
}

// ─────────────────────────────────────────────────────────────
// 命令输出格式化
// ─────────────────────────────────────────────────────────────

pub struct CommandOutput {
    theme: ThemeColors,
}

impl CommandOutput {
    pub fn new(theme_name: &str) -> Self {
        Self {
            theme: ThemeColors::from_config(theme_name),
        }
    }

    pub fn status(&self, running: bool, pid: Option<u32>) {
        let width = 44;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);

        if running {
            let cat = pick_random(kaomoji::CAT);
            eprintln!("  {}{}  {} 正在运行 {}",
                self.theme.primary, box_drawing::VERTICAL,
                colorize("Hakimi Hub", self.theme.accent),
                cat);
            if let Some(p) = pid {
                eprintln!("  {}{}  PID: {}",
                    self.theme.primary, box_drawing::VERTICAL,
                    colorize(&p.to_string(), self.theme.glow));
            }
        } else {
            eprintln!("  {}{}  {} 未运行 {}",
                self.theme.primary, box_drawing::VERTICAL,
                colorize("Hakimi Hub", self.theme.soft),
                pick_random(kaomoji::SLEEPY));
        }

        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);
        eprintln!("{}", RESET);
    }

    pub fn stopped(&self, pid: Option<u32>) {
        let width = 44;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);

        if let Some(p) = pid {
            eprintln!("  {}{}  已向进程 {} 发送停止信号 {}",
                self.theme.primary, box_drawing::VERTICAL,
                colorize(&p.to_string(), self.theme.glow),
                pick_random(kaomoji::CAT));
        } else {
            eprintln!("  {}{}  未找到运行中的实例 {}",
                self.theme.primary, box_drawing::VERTICAL,
                pick_random(kaomoji::WORKING));
        }

        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);
        eprintln!("{}", RESET);
    }

    pub fn ca_exported(&self, path: &str) {
        let width = 44;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);
        eprintln!("  {}{}  CA 证书已导出 {}",
            self.theme.primary, box_drawing::VERTICAL,
            pick_random(kaomoji::SUCCESS));
        eprintln!("  {}{}  路径: {}",
            self.theme.primary, box_drawing::VERTICAL,
            colorize(path, self.theme.accent));
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);
        eprintln!();
        eprintln!("  {} 将此证书添加到系统信任链即可启用 MITM 拦截", colorize("提示:", YELLOW));
        eprintln!("{}", RESET);
    }

    pub fn git_setup(&self) {
        let width = 44;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);
        eprintln!("  {}{}  Git 代理配置成功 {}",
            self.theme.primary, box_drawing::VERTICAL,
            pick_random(kaomoji::LOVE));
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);
        eprintln!();
        eprintln!("  {} git clone/push/pull 将通过 Hakimi Hub 加速", colorize("提示:", YELLOW));
        eprintln!("{}", RESET);
    }

    pub fn git_teardown(&self) {
        let width = 44;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);
        eprintln!("  {}{}  Git 代理配置已恢复 {}",
            self.theme.primary, box_drawing::VERTICAL,
            pick_random(kaomoji::CAT));
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);
        eprintln!("{}", RESET);
    }

    pub fn config_init(&self, path: &str) {
        let width = 44;
        let line = divider(&self.theme, width);

        eprintln!();
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line);
        eprintln!("  {}{}  配置模板已生成 {}",
            self.theme.primary, box_drawing::VERTICAL,
            pick_random(kaomoji::SUCCESS));
        eprintln!("  {}{}  路径: {}",
            self.theme.primary, box_drawing::VERTICAL,
            colorize(path, self.theme.accent));
        eprintln!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line);
        eprintln!();
        eprintln!("  {} 编辑配置文件自定义行为", colorize("提示:", YELLOW));
        eprintln!("{}", RESET);
    }

    pub fn error(&self, message: &str) {
        let width = 44;
        let line = box_drawing::HORIZONTAL.repeat(width);

        eprintln!();
        eprintln!("  {}{}{}", RED, box_drawing::TOP_LEFT, line);
        eprintln!("  {}{}  错误: {}", RED, box_drawing::VERTICAL, message);
        eprintln!("  {}{}{}", RED, box_drawing::BOTTOM_LEFT, line);
        eprintln!("{}", RESET);
    }

    pub fn warning(&self, message: &str) {
        eprintln!();
        eprintln!("  {} {} {}", colorize("!", YELLOW), message, pick_random(kaomoji::WORKING));
        eprintln!("{}", RESET);
    }

    pub fn success(&self, message: &str) {
        eprintln!();
        eprintln!("  {} {} {}",
            colorize("*", GREEN),
            message,
            pick_random(kaomoji::SUCCESS));
        eprintln!("{}", RESET);
    }
}

// ─────────────────────────────────────────────────────────────
// 道别消息
// ─────────────────────────────────────────────────────────────

pub fn goodbye(theme_name: &str) {
    let theme = ThemeColors::from_config(theme_name);
    let width = 44;
    let line = divider(&theme, width);

    eprintln!();
    eprintln!("  {}{}{}", theme.primary, box_drawing::TOP_LEFT, line);
    eprintln!("  {}│{}  正在停止服务...", theme.primary, RESET);
    eprintln!("  {}│{}    [OK] 关闭连接", theme.primary, RESET);
    eprintln!("  {}│{}    [OK] 恢复系统代理", theme.primary, RESET);
    eprintln!("  {}│{}    [OK] 清理临时文件", theme.primary, RESET);
    eprintln!("  {}├{}{}", theme.primary, line, RESET);
    eprintln!("  {}│{}  {} {} 已停止 {}",
        theme.primary, RESET, bold("Hakimi Hub"), colorize("(u_u)", theme.accent), RESET);
    eprintln!("  {}{}{}", theme.primary, box_drawing::BOTTOM_LEFT, line);
    eprintln!("{}", RESET);
}

// ─────────────────────────────────────────────────────────────
// 实时状态面板
// ─────────────────────────────────────────────────────────────

// 流量趋势图采样点数量
const SPARKLINE_SAMPLES: usize = 20;

// 猫的多种造型（根据主题匹配）
fn get_cat_frames_by_theme(theme: &ThemeColors) -> &[&str] {
    // 根据主题颜色匹配对应的猫造型
    if theme.primary == PINK_PRIMARY {
        // 粉粉喵
        &[
            "(=^･ω･^=)ノ",
            "(=^･ω･^=) ",
            "ヾ(=^･ω･^=)",
            "(=^･ω･^=) ",
        ]
    } else if theme.primary == MORNING_PRIMARY {
        // 阳光喵
        &[
            "(≧ω≦)ノ",
            "(≧ω≦) ",
            "ヾ(≧ω≦)",
            "(≧ω≦) ",
        ]
    } else if theme.primary == NOON_PRIMARY {
        // 可可喵
        &[
            "(˘ω˘)ノ",
            "(˘ω˘) ",
            "ヾ(˘ω˘)",
            "(˘ω˘) ",
        ]
    } else if theme.primary == NIGHT_PRIMARY {
        // 代码喵
        &[
            "(◉ω◉)ノ",
            "(◉ω◉) ",
            "ヾ(◉ω◉)",
            "(◉ω◉) ",
        ]
    } else {
        // 默认
        &[
            "(=^･ω･^=)ノ",
            "(=^･ω･^=) ",
            "ヾ(=^･ω･^=)",
            "(=^･ω･^=) ",
        ]
    }
}

pub struct RuntimePanel {
    metrics: Arc<Metrics>,
    theme: ThemeColors,
    start_time: Instant,
    // 流量历史记录（用于趋势图）
    rate_history: std::sync::Mutex<VecDeque<u64>>,
    // 固定面板行数（必须与实际打印行数一致）
    panel_lines: usize,
    initialized: std::sync::atomic::AtomicBool,
    frame: AtomicUsize,
    // 猫的位置（用于真实位移）
    cat_position: std::sync::Mutex<usize>,
}

impl RuntimePanel {
    pub fn new(
        metrics: Arc<Metrics>,
        _mirror_health: Arc<crate::intercepts::mirror_health::MirrorHealthTracker>,
        theme_name: &str,
    ) -> Self {
        Self {
            metrics,
            theme: ThemeColors::from_config(theme_name),
            start_time: Instant::now(),
            rate_history: std::sync::Mutex::new(VecDeque::with_capacity(SPARKLINE_SAMPLES)),
            panel_lines: 10,
            initialized: std::sync::atomic::AtomicBool::new(false),
            frame: AtomicUsize::new(0),
            cat_position: std::sync::Mutex::new(0),
        }
    }

    pub fn update_last_request(&self, _domain: &str) {
        // 简化后不再显示最后请求
    }

    // 生成动态行走的猫（根据主题选择造型）
    fn render_walking_cat(&self) -> String {
        let frame = self.frame.load(Ordering::Relaxed);

        // 更新猫的位置（每帧移动一格）
        if let Ok(mut pos) = self.cat_position.lock() {
            *pos = (*pos + 1) % 30;
        }

        let pos = self.cat_position.lock()
            .map(|p| *p)
            .unwrap_or(0);

        // 根据主题选择猫的造型
        let cat_frames = get_cat_frames_by_theme(&self.theme);
        let cat = cat_frames[frame % cat_frames.len()];

        // 真实位移
        let spaces = " ".repeat(pos);
        let trail_width = 30;
        let cat_len = cat.chars().count();
        let trail_spaces = if pos + cat_len < trail_width {
            " ".repeat(trail_width - pos - cat_len)
        } else {
            String::new()
        };

        format!("{}{}{}{}", spaces, colorize(cat, self.theme.glow), trail_spaces, RESET)
    }

    fn format_bytes(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;
        const GB: u64 = 1024 * MB;

        if bytes >= GB {
            format!("{:.1} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.1} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} B", bytes)
        }
    }

    fn format_rate(bytes_per_sec: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;

        if bytes_per_sec >= MB {
            format!("{:.1} MB/s", bytes_per_sec as f64 / MB as f64)
        } else if bytes_per_sec >= KB {
            format!("{:.1} KB/s", bytes_per_sec as f64 / KB as f64)
        } else if bytes_per_sec > 0 {
            format!("{} B/s", bytes_per_sec)
        } else {
            "0".to_string()
        }
    }

    fn format_uptime(elapsed: Duration) -> String {
        let secs = elapsed.as_secs();
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }

    // 生成 sparkline 趋势图
    fn render_sparkline(&self, current_rate: u64) -> String {
        // 更新历史记录
        if let Ok(mut history) = self.rate_history.lock() {
            if history.len() >= SPARKLINE_SAMPLES {
                history.pop_front();
            }
            history.push_back(current_rate);
        }

        // 获取历史数据
        let history = self.rate_history.lock()
            .map(|h| h.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();

        if history.is_empty() {
            // 返回空白的趋势图占位符，确保行数固定
            return " ".repeat(SPARKLINE_SAMPLES);
        }

        // 找最大值，避免除零
        let max_val = history.iter().max().copied().unwrap_or(1).max(1);

        // Unicode block 字符: ▁▂▃▄▅▆▇█
        let sparks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

        history.iter().map(|&v| {
            let normalized = v as f32 / max_val as f32;
            let idx = (normalized * 7.0).round() as usize;
            format!("{}{}", self.theme.accent, sparks[idx.min(7)])
        }).collect()
    }

    pub fn print_panel(&self) {
        let stats = self.metrics.snapshot();

        // 获取实时速率
        let (rate_sent, rate_recv) = self.metrics.get_rate_and_reset();

        let frame = self.frame.fetch_add(1, Ordering::Relaxed) % 4;

        let width = 50;
        let line = gradient_divider(&self.theme, width, frame);

        // 预分配固定数量的行
        let mut lines: Vec<String> = Vec::with_capacity(self.panel_lines);

        // 1. 顶部线
        lines.push(format!("  {}{}{}", self.theme.primary, box_drawing::TOP_LEFT, line));

        // 2. 标题（带动态行走的猫）
        let walking_cat = self.render_walking_cat();
        lines.push(format!("  {}{}  {} {}{}",
            self.theme.primary, box_drawing::VERTICAL,
            bold("运行中"), colorize("Hakimi Hub", self.theme.accent),
            walking_cat));

        // 3. 分隔线
        lines.push(format!("  {}{}{}", self.theme.primary, box_drawing::LEFT_TEE, line));

        // 4. 速率
        let sent_rate = Self::format_rate(rate_sent);
        let recv_rate = Self::format_rate(rate_recv);
        lines.push(format!("  {}{}  速率: ↑ {:>10}  ↓ {:>10}",
            self.theme.primary, box_drawing::VERTICAL,
            colorize(&sent_rate, self.theme.accent),
            colorize(&recv_rate, self.theme.secondary)));

        // 5. 趋势图（始终打印，保持行数固定）
        let sparkline = self.render_sparkline(rate_recv);
        lines.push(format!("  {}{}        {}{}",
            self.theme.primary, box_drawing::VERTICAL,
            sparkline, RESET));

        // 6. 累计流量
        let total_sent = Self::format_bytes(stats.bytes_sent);
        let total_recv = Self::format_bytes(stats.bytes_received);
        lines.push(format!("  {}{}  流量: ↑ {:>10}  ↓ {:>10}",
            self.theme.primary, box_drawing::VERTICAL,
            colorize(&total_sent, self.theme.accent),
            colorize(&total_recv, self.theme.secondary)));

        // 7. 分隔线
        lines.push(format!("  {}{}{}", self.theme.primary, box_drawing::LEFT_TEE, line));

        // 8. 运行时间
        let uptime = Self::format_uptime(self.start_time.elapsed());
        lines.push(format!("  {}{}  运行: {}",
            self.theme.primary, box_drawing::VERTICAL,
            colorize(&uptime, self.theme.glow)));

        // 9. 底部线
        lines.push(format!("  {}{}{}", self.theme.primary, box_drawing::BOTTOM_LEFT, line));

        // 10. 状态提示
        let face = if rate_recv > 2_000_000 {  // > 2 MB/s
            colorize("(o_o)", YELLOW)
        } else if rate_recv > 500_000 {  // > 500 KB/s
            colorize("(^_^)", self.theme.accent)
        } else {
            colorize("(-_-)", DIM)
        };
        lines.push(format!("   {}  版本 {} | Ctrl+C 停止",
            face, colorize(env!("CARGO_PKG_VERSION"), self.theme.glow)));

        // 确保行数与 panel_lines 一致
        assert_eq!(lines.len(), self.panel_lines, "Panel lines mismatch!");

        // 强制刷新输出
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();

        if self.initialized.swap(true, std::sync::atomic::Ordering::Relaxed) {
            // 后续刷新：上移并重绘
            write!(stderr, "\x1B[{}A", self.panel_lines).ok();
            for line_content in &lines {
                writeln!(stderr, "\r\x1B[K{}", line_content).ok();
            }
        } else {
            // 首次显示：运行面板直接在当前位置显示（启动成功框已渐隐消失）
            for line_content in &lines {
                writeln!(stderr, "{}", line_content).ok();
            }
        }
        stderr.flush().ok();
    }
}

// ─────────────────────────────────────────────────────────────
// 检测终端颜色支持
// ─────────────────────────────────────────────────────────────

pub fn supports_color() -> bool {
    anstream::AutoStream::choice(&std::io::stderr()) != anstream::ColorChoice::Never
}
