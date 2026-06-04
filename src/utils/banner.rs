use rand::{seq::SliceRandom, Rng};


fn supports_color() -> bool {
    anstream::AutoStream::choice(&std::io::stderr()) != anstream::ColorChoice::Never
}


const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";

// 粉色系 (默认)
const PINK_PRIMARY: &str = "\x1b[38;5;219m";
const PINK_SECONDARY: &str = "\x1b[38;5;213m";
const PINK_ACCENT: &str = "\x1b[38;5;206m";
const PINK_GLOW: &str = "\x1b[38;5;177m";
const PINK_SOFT: &str = "\x1b[38;5;225m";

// 晨光系
const MORNING_PRIMARY: &str = "\x1b[38;5;220m";
const MORNING_SECONDARY: &str = "\x1b[38;5;209m";
const MORNING_ACCENT: &str = "\x1b[38;5;214m";
const MORNING_GLOW: &str = "\x1b[38;5;178m";
const MORNING_SOFT: &str = "\x1b[38;5;222m";

// 薄荷系
const NOON_PRIMARY: &str = "\x1b[38;5;123m";
const NOON_SECONDARY: &str = "\x1b[38;5;122m";
const NOON_ACCENT: &str = "\x1b[38;5;117m";
const NOON_GLOW: &str = "\x1b[38;5;154m";
const NOON_SOFT: &str = "\x1b[38;5;194m";

// 深夜系（代码喵）- 深蓝/青色系
const NIGHT_PRIMARY: &str = "\x1b[38;5;39m";      // 深蓝
const NIGHT_SECONDARY: &str = "\x1b[38;5;45m";    // 青色
const NIGHT_ACCENT: &str = "\x1b[38;5;51m";       // 亮青
const NIGHT_GLOW: &str = "\x1b[38;5;81m";         // 浅蓝
const NIGHT_SOFT: &str = "\x1b[38;5;117m";        // 淡青


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
    pub primary: &'static str,
    pub secondary: &'static str,
    pub accent: &'static str,
    pub glow: &'static str,
    pub soft: &'static str,
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


// 旋转指示器
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// Logo 行
fn get_logo_lines() -> Vec<&'static str> {
    vec![
        "  ██╗  ██╗ █████╗ ███╗   ██╗██╗██╗  ██╗",
        "  ██║  ██║██╔══██╗████╗  ██║██║╚██╗██╔╝",
        "  ███████║███████║██╔██╗ ██║██║ ╚███╔╝ ",
        "  ██╔══██║██╔══██║██║╚██╗██║██║ ██╔██╗ ",
        "  ██║  ██║██║  ██║██║ ╚████║██║██╔╝ ██╗",
        "  ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝╚═╝  ╚═╝",
    ]
}

// Logo 单字符颜色化
fn colorize_logo_char(ch: char, idx: usize, theme: &ThemeColors) -> String {
    let gradient = [theme.primary, theme.secondary, theme.accent, theme.glow, theme.soft];
    let color = gradient[idx % gradient.len()];
    format!("{}{}{}", color, ch, RESET)
}

// 猫咪造型定义（基础版, 眨眼版）
type CatShape = (&'static [&'static str], &'static [&'static str]);

// 粉色系猫咪
fn get_pink_cats() -> Vec<CatShape> {
    vec![
        // 粉粉喵
        (
            &[
                "     /\\_/\\  ",
                "    (≧◡≦)    ",
                "   /  ♡♡  \\ ",
                "  ( 粉粉喵~ ) ",
                "   \\  ◡  /  ",
            ],
            &[
                "     /\\_/\\  ",
                "    (≧‿≦)    ",
                "   /  ♡♡  \\ ",
                "  ( 粉粉喵~ ) ",
                "   \\  ◡  /  ",
            ],
        ),
        // 恋爱喵
        (
            &[
                "    /\\_/\\   ",
                "   (♥ω♥)    ",
                "  /  桃花  \\ ",
                " (  恋爱喵  ) ",
            ],
            &[
                "    /\\_/\\   ",
                "   (♥‿♥)    ",
                "  /  桃花  \\ ",
                " (  恋爱喵  ) ",
            ],
        ),
        // 甜心喵
        (
            &[
                "     /\\_/\\   ",
                "    (˘ω˘)♡   ",
                "   ╭─┬─┬─┬─╮ ",
                "   │ 甜心喵 │ ",
                "   ╰─┴─┴─┴─╯ ",
            ],
            &[
                "     /\\_/\\   ",
                "    (˘‿˘)♡   ",
                "   ╭─┬─┬─┬─╮ ",
                "   │ 甜心喵 │ ",
                "   ╰─┴─┴─┴─╯ ",
            ],
        ),
    ]
}

// 晨光系猫咪
fn get_morning_cats() -> Vec<CatShape> {
    vec![
        // 阳光喵
        (
            &[
                "     /\\_/\\  ",
                "    (≧ω≦)    ",
                "   /  ^_^  \\ ",
                "  ( 阳光喵  ) ",
            ],
            &[
                "     /\\_/\\  ",
                "    (≧‿≦)    ",
                "   /  ^_^  \\ ",
                "  ( 阳光喵  ) ",
            ],
        ),
        // 早起喵
        (
            &[
                "     /\\_/\\  ",
                "    (≧◡≦)    ",
                "   /  ~~~  \\ ",
                "  ( 早起喵~ ) ",
                "   \\  ◡  /  ",
            ],
            &[
                "     /\\_/\\  ",
                "    (≧‿≦)    ",
                "   /  ~~~  \\ ",
                "  ( 早起喵~ ) ",
                "   \\  ◡  /  ",
            ],
        ),
        // 绿豆喵
        (
            &[
                "  ╭──────╮  ",
                "  │ 南  北 │  ",
                "  │ 绿豆喵 │  ",
                "  ╰──┬┬──╯  ",
                "   (・ω・)   ",
            ],
            &[
                "  ╭──────╮  ",
                "  │ 南  北 │  ",
                "  │ 绿豆喵 │  ",
                "  ╰──┬┬──╯  ",
                "   (・‿・)   ",
            ],
        ),
    ]
}

// 薄荷系猫咪
fn get_noon_cats() -> Vec<CatShape> {
    vec![
        // 可可喵
        (
            &[
                "     /\\_/\\  ",
                "    (˘ω˘)    ",
                "   ╭─┬─┬─┬─╮ ",
                "   │  喵  │ ",
                "   ╰─┴─┴─┴─╯ ",
            ],
            &[
                "     /\\_/\\  ",
                "    (˘‿˘)    ",
                "   ╭─┬─┬─┬─╮ ",
                "   │  喵  │ ",
                "   ╰─┴─┴─┴─╯ ",
            ],
        ),
        // Nya喵
        (
            &[
                "     /\\_/\\  ",
                "    (≧◡≦)    ",
                "   /  ~*~  \\ ",
                "  (  Nya~  ) ",
                "   \\  ◡  /  ",
            ],
            &[
                "     /\\_/\\  ",
                "    (≧‿≦)    ",
                "   /  ~*~  \\ ",
                "  (  Nya~  ) ",
                "   \\  ◡  /  ",
            ],
        ),
        // 星星喵
        (
            &[
                "    /\\_/\\   ",
                "   (★ω★)    ",
                "  ╭─╮ ╭─╮  ",
                "  │*│ │*│  ",
                "  ╰─╯ ╰─╯  ",
            ],
            &[
                "    /\\_/\\   ",
                "   (★‿★)    ",
                "  ╭─╮ ╭─╮  ",
                "  │*│ │*│  ",
                "  ╰─╯ ╰─╯  ",
            ],
        ),
    ]
}

// 深夜系猫咪
fn get_night_cats() -> Vec<CatShape> {
    vec![
        // 代码喵 - 笔记本敲代码
        (
            &[
                "   ╭───────╮  ",
                "   │>code_ │  ",
                "   │ ████  │  ",
                "   │ ████  │  ",
                "   ╰───────╯  ",
                "   (◉ω◉)     ",
                "   /|  |\\    ",
            ],
            &[
                "   ╭───────╮  ",
                "   │>code_ │  ",
                "   │ ████  │  ",
                "   │ ████  │  ",
                "   ╰───────╯  ",
                "   (◉‿◉)     ",
                "   /|  |\\    ",
            ],
        ),
        // 星夜喵
        (
            &[
                "    /\\_/\\   ",
                "   (★ω★)    ",
                "  ╭──────╮  ",
                "  │ 星夜喵 │  ",
                "  ╰──────╯  ",
            ],
            &[
                "    /\\_/\\   ",
                "   (★‿★)    ",
                "  ╭──────╮  ",
                "  │ 星夜喵 │  ",
                "  ╰──────╯  ",
            ],
        ),
        // 困困喵
        (
            &[
                "    /\\_/\\   ",
                "   (-ω-)    ",
                "  /  z Z  \\ ",
                " ( 困困喵  ) ",
            ],
            &[
                "    /\\_/\\   ",
                "   (-‿-)    ",
                "  /  z Z  \\ ",
                " ( 困困喵  ) ",
            ],
        ),
    ]
}

// 通用猫咪（任何主题都可能出现的）
fn get_common_cats() -> Vec<CatShape> {
    vec![
        // 基础喵
        (
            &[
                "     /\\_/\\  ",
                "    (=^･ω･^=) ",
                "  /       \\ ",
                " (  喵喵  ) ",
            ],
            &[
                "     /\\_/\\  ",
                "    (=^･‿･^=) ",
                "  /       \\ ",
                " (  喵喵  ) ",
            ],
        ),
        // 小萌喵
        (
            &[
                "    ∩∩    ",
                "   (・ω・)   ",
                "   ⊂彡☆))д´) ",
            ],
            &[
                "    ∩∩    ",
                "   (・‿・)   ",
                "   ⊂彡☆))д´) ",
            ],
        ),
    ]
}

// 猫咪（带眨眼变体）- 根据主题选择，支持多猫咪随机
fn get_cat_with_blink(theme: &ThemeColors) -> (Vec<String>, Vec<String>) {
    let mut rng = rand::thread_rng();

    // 70% 概率选主题专属猫咪，30% 选通用猫咪
    let cats = if rng.gen::<f32>() < 0.7 {
        if theme.primary == PINK_PRIMARY {
            get_pink_cats()
        } else if theme.primary == MORNING_PRIMARY {
            get_morning_cats()
        } else if theme.primary == NOON_PRIMARY {
            get_noon_cats()
        } else if theme.primary == NIGHT_PRIMARY {
            get_night_cats()
        } else {
            get_common_cats()
        }
    } else {
        get_common_cats()
    };

    // 随机选择一个猫咪
    let (base, blink) = cats.choose(&mut rng).unwrap();

    let base_vec: Vec<String> = base.iter().map(|s| s.to_string()).collect();
    let blink_vec: Vec<String> = blink.iter().map(|s| s.to_string()).collect();

    (base_vec, blink_vec)
}


// 动态分隔线（带动画帧）
fn animated_divider_frame(theme: &ThemeColors, frame: usize) -> String {
    let width = 52;
    let gradient = [theme.primary, theme.secondary, theme.accent, theme.glow, theme.soft];
    let chars = ["━", "╾", "─", "╼"];

    let mut line = String::new();
    for i in 0..width {
        // 颜色渐变
        let color_idx = (i * gradient.len() / width) % gradient.len();
        // 字符动画：基于帧和位置选择字符
        let char_idx = (i + frame) % chars.len();
        line.push_str(gradient[color_idx]);
        line.push_str(chars[char_idx]);
    }
    line.push_str(RESET);
    line
}

// 静态分隔线
fn divider(theme: &ThemeColors) -> String {
    animated_divider_frame(theme, 0)
}


/// 打印启动 banner（带动画）
pub fn print_banner() {
    print_banner_with_theme("pink");
}

/// 打印启动 banner，使用配置中指定的主题
pub fn print_banner_with_theme(theme_name: &str) {
    if !supports_color() {
        return;
    }

    let theme = ThemeColors::from_config(theme_name);

    // 清屏并移动光标到顶部
    eprint!("\x1B[2J\x1B[H");

    // 动画 Phase 1: Logo 打字机效果（逐字符）
    let logo_lines = get_logo_lines();
    let mut char_count = 0;

    for line in &logo_lines {
        eprint!("  ");
        for ch in line.chars() {
            let colored = if ch != ' ' {
                char_count += 1;
                colorize_logo_char(ch, char_count, &theme)
            } else {
                " ".to_string()
            };
            eprint!("{}", colored);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        eprintln!();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    // 动画 Phase 2: 旋转指示器 + 分隔线动画
    eprintln!();
    for frame in 0..5 {
        // 清除上一行
        eprint!("\x1B[A\x1B[K");

        let spinner = SPINNER[frame % SPINNER.len()];
        let div = animated_divider_frame(&theme, frame);
        eprintln!("  {}{}  {}  加载中...", theme.glow, spinner, div);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // 清除旋转指示器行
    eprint!("\x1B[A\x1B[K");

    // 空行
    eprintln!();

    // 动画 Phase 3: 猫咪眨眼效果
    let (cat_base, cat_blink) = get_cat_with_blink(&theme);

    // 先打印基础猫咪
    for line in &cat_base {
        eprintln!("{}{}", theme.glow, line);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // 眨眼动画（2 次，减少次数）
    for _ in 0..2 {
        std::thread::sleep(std::time::Duration::from_millis(150));

        // 清除猫咪区域并重绘眨眼版本
        for _ in 0..cat_base.len() {
            eprint!("\x1B[A\x1B[K");
        }

        for line in &cat_blink {
            eprintln!("{}{}", theme.glow, line);
        }

        std::thread::sleep(std::time::Duration::from_millis(50));

        // 恢复基础猫咪
        for _ in 0..cat_blink.len() {
            eprint!("\x1B[A\x1B[K");
        }

        for line in &cat_base {
            eprintln!("{}{}", theme.glow, line);
        }
    }

    // 分隔线
    let div = divider(&theme);
    eprintln!("  {}", div);
}

pub fn print_goodbye() {
    print_goodbye_with_theme("pink");
}

pub fn print_goodbye_with_theme(theme_name: &str) {
    if !supports_color() {
        return;
    }

    let theme = ThemeColors::from_config(theme_name);
    let div = divider(&theme);

    let faces = &["(=^･ω･^=)ノ", "(=^･ｪ･^=) zzZ", "ฅ^•ﻌ•^ฅ"];

    let mut rng = rand::thread_rng();
    let face = faces.choose(&mut rng).unwrap();

    eprintln!();
    eprintln!("  {}", div);
    eprintln!("  {primary}{face}{reset} {dim}下次见喵~{reset}",
              primary = theme.primary, dim = DIM, reset = RESET, face = face);
    eprintln!("  {}", div);
    eprintln!();
}

pub fn print_status(status: &str) {
    print_status_with_theme(status, "pink");
}

pub fn print_status_with_theme(status: &str, theme_name: &str) {
    if !supports_color() {
        eprintln!("  > {}", status);
        return;
    }
    let theme = ThemeColors::from_config(theme_name);
    eprintln!("  {primary}>{reset} {status}",
              primary = theme.primary, reset = RESET, status = status);
}

pub fn print_success(msg: &str) {
    if !supports_color() {
        eprintln!("  [+] {}", msg);
        return;
    }
    eprintln!("  {green}[+]{reset} {msg}",
              green = GREEN, reset = RESET, msg = msg);
}

pub fn print_warning(msg: &str) {
    if !supports_color() {
        eprintln!("  [!] {}", msg);
        return;
    }
    eprintln!("  {yellow}[!]{reset} {msg}",
              yellow = YELLOW, reset = RESET, msg = msg);
}

pub fn print_error(msg: &str) {
    if !supports_color() {
        eprintln!("  [-] {}", msg);
        return;
    }
    eprintln!("  {red}[-]{reset} {msg}",
              red = RED, reset = RESET, msg = msg);
}


pub fn print_info(msg: &str) {
    print_info_with_theme(msg, "pink");
}

pub fn print_info_with_theme(msg: &str, theme_name: &str) {
    if !supports_color() {
        eprintln!("  [*] {}", msg);
        return;
    }
    let theme = ThemeColors::from_config(theme_name);
    eprintln!("  {primary}[*]{reset} {msg}",
              primary = theme.primary, reset = RESET, msg = msg);
}
