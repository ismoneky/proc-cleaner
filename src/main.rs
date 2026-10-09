#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 进程清理器 —— 列出占用内存的进程，勾选后结束。
//!
//! 相对最初版本改了三处（原因见 README.md）：
//!   1. `BOOL` 改从 `windows::core` 导入 —— windows crate 0.60 起
//!      `Win32::Foundation::BOOL` 已被移除，原写法在 0.60+ 上无法编译。
//!   2. 保护名单改为「归一化后比较」—— 原名混用 `"system"` 与 `"smss.exe"`
//!      两种写法，一旦与 sysinfo 实际返回的格式不一致，保护会静默失效。
//!   3. 确认弹窗改为「快照式」—— 弹窗打开后再改动勾选，不会影响真正被杀的目标。
//!
//! 另外补了一个 panic 日志：release 构建没有控制台，崩了会什么都不显示，
//! 日志文件写在 exe 同目录，方便排查。

use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

// ---------------------------------------------------------------------------
// 系统关键进程黑名单：结束这些进程会导致系统不稳定 / 黑屏 / 直接重启。
//
// 【注意】这里统一写成「归一化之后」的形式（全小写、不带 .exe）。
// 比较时两边都会过 normalize()，所以 sysinfo 返回 "lsass.exe" 还是 "lsass"
// 都能拦住，不会被绕过去。
// ---------------------------------------------------------------------------
const PROTECTED: &[&str] = &[
    "system",
    "system idle process",
    "registry",
    "memory compression",
    "secure system",
    "smss",
    "csrss",
    "wininit",
    "winlogon",
    "logonui",
    "lsaiso",
    "services",
    "lsass",
    "svchost",
    "dwm",
    "fontdrvhost",
    "audiodg",
    "explorer", // 结束会丢失桌面和任务栏（会自动重启，但体验很差）
    "sihost",
    "ctfmon",
    "taskhostw",
    "startmenuexperiencehost",
    "shellexperiencehost",
    "searchhost",
    "msmpeng",
    "securityhealthservice",
];

/// 把进程名归一化：去首尾空白、转小写、去掉 `.exe` 后缀。
///
/// sysinfo 在不同平台/版本上返回的名字带不带 `.exe` 并不一致，
/// 直接拿原始字符串比对一个硬编码名单，是「保护看起来写了、其实没生效」的
/// 经典成因。
fn normalize(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

fn is_protected(name: &str) -> bool {
    let n = normalize(name);
    PROTECTED.iter().any(|p| *p == n)
}

// ---------------------------------------------------------------------------
// 枚举顶层可见窗口，得到「哪些 PID 有窗口」以及窗口标题
// ---------------------------------------------------------------------------
#[derive(Default)]
struct WindowInfo {
    pids: HashSet<u32>,
    titles: HashMap<u32, String>,
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let info = &mut *(lparam.0 as *mut WindowInfo);

    if IsWindowVisible(hwnd).as_bool() {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        if pid != 0 {
            let len = GetWindowTextLengthW(hwnd);
            if len > 0 {
                let mut buf = vec![0u16; len as usize + 1];
                let n = GetWindowTextW(hwnd, &mut buf);
                if n > 0 {
                    let title = String::from_utf16_lossy(&buf[..n as usize]);
                    if !title.trim().is_empty() {
                        info.pids.insert(pid);
                        info.titles.entry(pid).or_insert(title);
                    }
                }
            }
        }
    }

    // windows crate 0.60+ 里 BOOL 是 windows::core 的元组结构体，构造方式就是 BOOL(1)。
    BOOL(1)
}

fn collect_windows() -> WindowInfo {
    let mut info = WindowInfo::default();
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_cb),
            LPARAM(&mut info as *mut WindowInfo as isize),
        );
    }
    info
}

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterKind {
    All,
    WithWindow,
    Background,
}

struct ProcRow {
    pid: Pid,
    name: String,
    exe: String,
    memory_mb: f64,
    cpu: f32,
    has_window: bool,
    title: String,
    protected: bool,
}

struct App {
    sys: System,
    rows: Vec<ProcRow>,
    selected: HashSet<Pid>,

    /// 自己的 PID，用来禁止「把自己结束掉」
    own_pid: Option<Pid>,

    search: String,
    filter: FilterKind,
    hide_protected: bool,
    min_memory_mb: f64,

    last_refresh: Instant,
    status: String,

    /// 非空 = 正在显示确认弹窗；内容就是「确定」后会被结束的那批 PID。
    /// 用快照而不是读 self.selected，是为了让弹窗上的数字和实际行为一致。
    confirm_pids: Vec<Pid>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);

        let mut app = Self {
            sys: System::new(),
            rows: Vec::new(),
            selected: HashSet::new(),
            own_pid: sysinfo::get_current_pid().ok(),
            search: String::new(),
            filter: FilterKind::All,
            hide_protected: true,
            min_memory_mb: 0.0,
            last_refresh: Instant::now(),
            status: String::new(),
            confirm_pids: Vec::new(),
        };
        app.refresh();
        app
    }

    // ---------------- 刷新进程列表 ----------------
    fn refresh(&mut self) {
        self.sys.refresh_processes(ProcessesToUpdate::All, true);
        let win = collect_windows();
        let own_pid = self.own_pid;

        let mut rows: Vec<ProcRow> = Vec::with_capacity(self.sys.processes().len());

        for (pid, p) in self.sys.processes() {
            let name = p.name().to_string_lossy().to_string();
            let pid_u32 = pid.as_u32();

            rows.push(ProcRow {
                pid: *pid,
                name: name.clone(),
                exe: p.exe().map(|e| e.display().to_string()).unwrap_or_default(),
                memory_mb: p.memory() as f64 / 1024.0 / 1024.0,
                cpu: p.cpu_usage(),
                has_window: win.pids.contains(&pid_u32),
                title: win.titles.get(&pid_u32).cloned().unwrap_or_default(),
                // 除了系统关键进程，也把自己标成受保护，避免自杀
                protected: is_protected(&name) || Some(*pid) == own_pid,
            });
        }

        // 默认按内存降序，占用大的排前面
        rows.sort_by(|a, b| {
            b.memory_mb
                .partial_cmp(&a.memory_mb)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        self.rows = rows;

        // 清理已经退出的进程的选择状态
        let alive: HashSet<Pid> = self.sys.processes().keys().copied().collect();
        self.selected.retain(|p| alive.contains(p));

        self.last_refresh = Instant::now();
    }

    // ---------------- 过滤 ----------------
    fn passes(&self, r: &ProcRow) -> bool {
        if self.hide_protected && r.protected {
            return false;
        }
        match self.filter {
            FilterKind::WithWindow if !r.has_window => return false,
            FilterKind::Background if r.has_window => return false,
            _ => {}
        }
        if r.memory_mb < self.min_memory_mb {
            return false;
        }
        if !self.search.trim().is_empty() {
            let s = self.search.trim().to_lowercase();
            if !r.name.to_lowercase().contains(&s) && !r.title.to_lowercase().contains(&s) {
                return false;
            }
        }
        true
    }

    fn visible_indices(&self) -> Vec<usize> {
        let mut v = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            if self.passes(r) {
                v.push(i);
            }
        }
        v
    }

    // ---------------- 结束进程 ----------------
    /// `targets` 由调用方给出（确认弹窗里的快照），本函数不再读 self.selected。
    fn do_kill(&mut self, targets: &[Pid]) {
        let own_pid = self.own_pid;
        let mut killed = 0usize;
        let mut skipped: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();

        for &pid in targets {
            let (name, row_protected) = match self.rows.iter().find(|r| r.pid == pid) {
                Some(r) => (r.name.clone(), r.protected),
                None => (pid.to_string(), false),
            };

            if row_protected || Some(pid) == own_pid {
                skipped.push(name);
                continue;
            }

            let Some(p) = self.sys.process(pid) else {
                continue; // 已经自己退出了
            };

            // 结束前按「实时名字」再核一次：PID 可能已经被复用成别的进程，
            // 只信刷新时缓存下来的名单有误杀风险。
            let live_name = p.name().to_string_lossy().to_string();
            if is_protected(&live_name) {
                skipped.push(live_name);
                continue;
            }

            if p.kill() {
                killed += 1;
            } else {
                failed.push(name);
            }
        }

        self.selected.clear();
        self.confirm_pids.clear();

        let mut msg = format!("已结束 {killed} 个进程");
        if !skipped.is_empty() {
            msg.push_str(&format!("；{} 个受保护已跳过", skipped.len()));
        }
        if !failed.is_empty() {
            msg.push_str(&format!(
                "；{} 个失败（需要以管理员身份运行）：{}",
                failed.len(),
                failed.join(", ")
            ));
        }
        self.status = msg;

        self.refresh();
    }
}

// ---------------------------------------------------------------------------
// 中文字体
// ---------------------------------------------------------------------------
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];

    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            // `.into()` 让这段代码在「font_data 存 FontData」和
            // 「font_data 存 Arc<FontData>」两种 egui 版本下都能编译。
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(data).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 每 2 秒自动刷新一次
        if self.last_refresh.elapsed() > Duration::from_secs(2) {
            self.refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(500));

        // ---------------- 顶部工具条 ----------------
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("进程清理器");
                ui.separator();
                if ui.button("🔄 刷新").clicked() {
                    self.refresh();
                }
                ui.separator();
                ui.label("搜索:");
                ui.add(egui::TextEdit::singleline(&mut self.search).desired_width(150.0));
                ui.separator();
                ui.label("类型:");
                ui.selectable_value(&mut self.filter, FilterKind::All, "全部");
                ui.selectable_value(&mut self.filter, FilterKind::WithWindow, "有窗口");
                ui.selectable_value(&mut self.filter, FilterKind::Background, "后台");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.hide_protected, "隐藏系统关键进程");
                ui.separator();
                ui.label("内存 ≥");
                ui.add(
                    egui::DragValue::new(&mut self.min_memory_mb)
                        .speed(1.0)
                        .range(0.0..=10000.0),
                );
                ui.label("MB");
            });
            ui.add_space(4.0);
        });

        // ---------------- 底部操作栏 ----------------
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let n = self.selected.len();
                let btn = egui::Button::new(
                    egui::RichText::new(format!("结束选中的 {n} 个进程"))
                        .color(egui::Color32::WHITE)
                        .strong(),
                )
                .fill(egui::Color32::from_rgb(178, 44, 44));

                if ui.add_enabled(n > 0, btn).clicked() {
                    // 打开弹窗的瞬间把目标固定下来
                    self.confirm_pids = self.selected.iter().copied().collect();
                }
                if ui.button("清空选择").clicked() {
                    self.selected.clear();
                }
                ui.separator();
                ui.label(self.status.as_str());
            });
            ui.add_space(4.0);
        });

        // ---------------- 中间列表 ----------------
        egui::CentralPanel::default().show(ctx, |ui| {
            let visible = self.visible_indices();

            ui.horizontal(|ui| {
                ui.label(format!(
                    "共 {} 个进程 / 显示 {} 个 / 已选 {} 个",
                    self.rows.len(),
                    visible.len(),
                    self.selected.len()
                ));
                ui.separator();
                if ui.button("全选显示项").clicked() {
                    for &i in &visible {
                        if !self.rows[i].protected {
                            self.selected.insert(self.rows[i].pid);
                        }
                    }
                }
                if ui.button("取消全选").clicked() {
                    for &i in &visible {
                        let pid = self.rows[i].pid;
                        self.selected.remove(&pid);
                    }
                }
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("proc_grid")
                        .striped(true)
                        .num_columns(6)
                        .spacing([14.0, 6.0])
                        .show(ui, |ui| {
                            // 表头
                            ui.label("");
                            ui.strong("进程名");
                            ui.strong("PID");
                            ui.strong("内存");
                            ui.strong("CPU");
                            ui.strong("窗口标题 / 路径");
                            ui.end_row();

                            for &i in &visible {
                                // 先把需要的字段复制出来，避免与 self.selected 的借用冲突
                                let (pid, name, mem, cpu, has_window, title, exe, prot) = {
                                    let r = &self.rows[i];
                                    (
                                        r.pid,
                                        r.name.clone(),
                                        r.memory_mb,
                                        r.cpu,
                                        r.has_window,
                                        r.title.clone(),
                                        r.exe.clone(),
                                        r.protected,
                                    )
                                };

                                // 勾选框
                                let mut checked = self.selected.contains(&pid);
                                if prot {
                                    ui.add_enabled(false, egui::Checkbox::without_text(&mut checked))
                                        .on_disabled_hover_text("系统关键进程，已锁定");
                                } else if ui.checkbox(&mut checked, "").changed() {
                                    if checked {
                                        self.selected.insert(pid);
                                    } else {
                                        self.selected.remove(&pid);
                                    }
                                }

                                // 进程名
                                let mut rt = egui::RichText::new(name.as_str());
                                if prot {
                                    rt = rt.weak();
                                }
                                ui.label(rt);

                                ui.label(pid.as_u32().to_string());
                                ui.label(format!("{mem:.1} MB"));
                                ui.label(format!("{cpu:.1}%"));

                                // 窗口标题（有窗口）或可执行路径（后台）
                                let desc = if has_window && !title.is_empty() {
                                    title
                                } else {
                                    exe
                                };
                                let short: String = desc.chars().take(60).collect();
                                ui.label(egui::RichText::new(short).weak())
                                    .on_hover_text(desc.as_str());

                                ui.end_row();
                            }
                        });
                });
        });

        // ---------------- 确认弹窗 ----------------
        if !self.confirm_pids.is_empty() {
            let n = self.confirm_pids.len();
            let mut do_it = false;
            let mut cancel = false;

            egui::Window::new("确认结束进程")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "确定要结束选中的 {n} 个进程吗？未保存的数据可能会丢失。"
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let yes = egui::Button::new(
                            egui::RichText::new("确定结束").color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(178, 44, 44));
                        if ui.add(yes).clicked() {
                            do_it = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                    });
                });

            if do_it {
                let targets = std::mem::take(&mut self.confirm_pids);
                self.do_kill(&targets);
            } else if cancel {
                self.confirm_pids.clear();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// release 构建是 windows_subsystem = "windows"，没有控制台，
// 一旦 panic 窗口直接消失、什么都看不到。这里把 panic 写到 exe 同目录的日志里。
// ---------------------------------------------------------------------------
fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        let path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("proc-cleaner-crash.log")))
            .unwrap_or_else(|| std::path::PathBuf::from("proc-cleaner-crash.log"));
        let _ = std::fs::write(&path, format!("panic: {info}\n"));
    }));
}

// ---------------------------------------------------------------------------
fn main() -> eframe::Result<()> {
    install_panic_logger();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([720.0, 420.0])
            .with_title("进程清理器"),
        ..Default::default()
    };

    eframe::run_native(
        "proc-cleaner",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
