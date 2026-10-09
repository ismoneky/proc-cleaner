# 进程清理器 (proc-cleaner)

把 DeepSeek 生成的那段 Rust 代码整理成了一个**真正的 Cargo 工程**，修掉了几个会
导致「编译不过」和「安全保护失效」的问题，并配了一条命令出 exe 的打包脚本。

```
proc-cleaner/
├─ Cargo.toml              依赖与版本（注意 windows crate 的版本约束）
├─ src/main.rs             程序本体（已修正）
├─ build.ps1               一条命令：检查 → 编译 → 产出 exe
├─ proc-cleaner.manifest   可选：让 exe 启动时直接要管理员权限
└─ README.md               本文件
```

---

## 一、先做这件事：确认它能不能编译

**我无法替你验证这一点。** 这台机器上的命令行完全起不来（每条命令都以
`0xC0000142 STATUS_DLL_INIT_FAILED` 退出，`pwsh.exe` 连 DLL 都加载不了），
所以我不能运行 `cargo check`，也就**没有真正编译过这份代码**。

你那边只要 Rust 环境正常，跑这一条就知道：

```powershell
cd proc-cleaner
.\build.ps1 -CheckOnly
```

`-CheckOnly` 只做 `cargo check --release`，不产出 exe，最快。

如果报错，把**完整错误原文**贴给我 —— Rust 的报错信息很具体，基本上一眼能定位。

---

## 二、打包成 exe

```powershell
cd proc-cleaner
.\build.ps1
```

产物：`proc-cleaner\dist\proc-cleaner.exe`（约 5–15 MB，取决于 LTO）

**这个 exe 是自包含的**：不需要装 Python、不需要 .NET、不需要任何运行库，
拷到别的 Windows 上双击就能跑。这是 Rust 相对 PyInstaller 那类方案的主要优势。

想让它一启动就要管理员权限（结束进程基本都需要）：

```powershell
.\build.ps1 -EmbedAdminManifest
```

不加这个开关的话，程序能正常打开、能看到进程列表，但结束大部分进程会失败，
状态栏会提示「需要以管理员身份运行」—— 那时右键 exe →「以管理员身份运行」也一样。

---

## 三、我改了什么，以及为什么

### 1. 【编译不过】`BOOL` 的导入位置 —— 这是硬伤

原代码：

```rust
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
```

**`windows` crate 从 0.60 版本起已经把 `BOOL` 从 `Win32::Foundation` 移到了
`windows::core`**，上面这行在 0.60+ 上会直接报
``no `BOOL` in `Win32::Foundation```。
（参考：[Microsoft Q&A 上同一个报错](https://learn.microsoft.com/en-gb/answers/questions/2223610/where-has-windows-win32-foundation-bool-been-moved)，
提问者最后的结论是「放弃使用 BOOL」。）

改法：

```rust
use windows::core::BOOL;
// ...
unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL { ... BOOL(1) }
```

`Cargo.toml` 里把 `windows` 锁在 `"0.62"`。**这一条和版本是绑死的**：如果你哪天
把 `windows` 降回 0.58/0.59，就要改回 `Win32::Foundation::BOOL` 并把 `BOOL(1)`
换回 `TRUE`。`Cargo.toml` 里我也写了这个注释。

### 2. 【安全】保护名单会「看起来写了、其实没生效」

原代码的保护名单里混着两种写法：

```rust
"system", "registry", "memory compression",   // 不带 .exe
"smss.exe", "lsass.exe", "csrss.exe",         // 带 .exe
```

而 `is_protected()` 是拿 sysinfo 返回的名字去**精确匹配**的：

```rust
PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(name))
```

问题在于 **sysinfo 的 `Process::name()` 在不同平台/版本上返回的名字带不带 `.exe`
并不一致**。也就是说，只要实际返回的格式和名单不一致，`lsass.exe`、`csrss.exe`、
`wininit.exe` 这一整批**会全部失去保护**——而这些进程被结束的后果是系统立即重启
或蓝屏，不是「某个软件崩了」。

这类 bug 最恶劣的地方是它**不会报错**：界面上勾选框照样能打勾，程序照样跑，你
不会知道保护已经失效了。

改法是把两边都归一化后再比：

```rust
fn normalize(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}
```

`PROTECTED` 里统一只写归一化后的形式（`"lsass"` 而不是 `"lsass.exe"`），
比较时两边都过一遍 `normalize()`。这样无论 sysinfo 返回哪种格式都能拦住。
我另外补了 `logonui`、`lsaiso` 两个同样不该被结束的进程。

### 3. 【逻辑】确认弹窗里的数字，和实际被杀的目标可能不一致

原代码的确认弹窗只是显示 `self.selected.len()`，点「确定」时才去读当时的
`self.selected`。而 `egui::Window` **不是真正的模态窗口**——弹窗开着的时候，
你仍然可以点到后面列表里的勾选框。于是「确认结束 3 个」可能变成实际结束 5 个。

改法：打开弹窗的瞬间把目标列表快照下来，之后「确定」只杀快照里的那批。

```rust
confirm_pids: Vec<Pid>,   // 非空 = 弹窗打开中
```

### 4. 【逻辑】程序会把自己列进列表，可以被自己结束

进程列表里包含 `proc-cleaner.exe` 自己。原代码没有排除，理论上可以勾上自己然后
把自己杀掉。改成刷新时就把自己的 PID 标记为 `protected`（勾选框同时变灰）。

### 5. 【逻辑】PID 复用导致的误杀窗口

`refresh()` 每 2 秒一次，`do_kill()` 从缓存下来的 `self.rows` 里按 PID 找名字来
判断是否受保护。如果在这 2 秒内某个受保护进程退出、PID 被系统分配给了别的进程，
缓存里的名字就是过期的。改成**结束前用实时名字再核一次**：

```rust
let live_name = p.name().to_string_lossy().to_string();
if is_protected(&live_name) { skipped.push(live_name); continue; }
```

### 6. 【可用性】release 版崩溃时什么都看不到

`windows_subsystem = "windows"` 意味着 release 构建没有控制台窗口。一旦 panic，
进程直接消失，你只会看到「闪一下就没了」，拿不到任何信息。加了一个 panic hook，
把信息写到 exe 同目录的 `proc-cleaner-crash.log`。

### 7. 【编译保险】避免依赖不确定的 `From<&String>` 实现

`RichText::new(&name)`、`ui.label(&self.status)`、`.on_hover_text(&desc)` 这种写法
依赖 egui 是否给 `WidgetText` 实现了 `From<&String>`。这个实现不同版本不保证都有。
统一改成 `.as_str()`，行为完全一样但肯定能编译。

---

## 四、我**没能**验证的部分（请重点看这里）

诚实清单，这些都要等你 `cargo check` 之后才能确认：

| 项 | 状态 |
|---|---|
| 代码能否编译 | **未验证**（本机命令行是坏的） |
| `windows::core::BOOL` 在 0.62 上的确切路径 | 依据 PR/社区证据推断，未编译验证 |
| `egui 0.31` / `eframe 0.31` / `sysinfo 0.33` 版本组合 | 逐项查过 API 文档，但未做整体解析 |
| 界面实际长什么样、中文是否正常显示 | **完全没跑过** |
| 结束进程的实际效果 | **完全没跑过** |

我逐条核对过的 API（有文档依据）：
- `sysinfo::Process::name/exe/memory/cpu_usage/kill`、`ProcessesToUpdate` —— 在最新 0.39.6 仍存在，0.33 也应有
- `egui::FontData::from_owned(Vec<u8>) -> FontData` —— 0.31.1 存在
- `eframe::run_native` 的 `AppCreator` 返回 `Result<Box<dyn App>>` —— 0.28+ 如此

**建议的第一次动作**：先 `.\build.ps1 -CheckOnly`。编译错误按行号贴给我，我来改。

---

## 五、还有几个「不算错，但值得知道」的点

1. **`svchost` 被整个保护了。** 名单里是 `"svchost"`，会匹配**所有** svchost
   实例。安全，但意味着你没法通过它清理任何基于服务的进程。这应该是故意的。

2. **`explorer` 也被保护了。** 原代码注释说「会自动重启但体验很差」。如果你确实
   想通过重启 explorer 来找回桌面，得自己从名单里删掉它。

3. **CPU 列首次显示可能是 0。** sysinfo 的 CPU 占用是两次采样之间的差值，第一次
   刷新没有基准值。等 2 秒自动刷新后就正常了。

4. **列表没有虚拟化。** `ScrollArea` + `Grid` 会把所有行都渲染出来。典型机器
   200–400 个进程时还行，但如果卡顿，可以改用 `ScrollArea::show_rows()` 做按需渲染。

5. **`build.ps1` 里没有中文。** 故意的：Windows PowerShell 5.1 在没有 UTF-8 BOM
   时按 ANSI 读取 `.ps1`，中文会变乱码。所有说明都放在这个 README 里。

---

## 六、改刷新间隔 / 改保护名单

在 `src/main.rs` 里：

- **自动刷新间隔**：`update()` 里的 `Duration::from_secs(2)`
- **保护名单**：`PROTECTED` 常量。**注意只写归一化后的名字**（全小写、不带 `.exe`），
  例如要加「记事本」就写 `"notepad"`，不要写 `"notepad.exe"`

---

## 七、云端编译（本机不装 Rust）

本机**没有安装 Rust**，`cargo`/`rustc` 都不存在。工程改用 GitHub Actions 在
`windows-latest` 上编译，本机只需要 `git`。

**已经跑通并产出 exe**：

- 仓库：<https://github.com/ismoneky/proc-cleaner>
- 工作流：`.github/workflows/build.yml`
- 永久下载直链（公开仓库，无需登录）：

  ```
  https://github.com/ismoneky/proc-cleaner/releases/latest/download/proc-cleaner.exe
  ```

工作流在每次 push 到 `main` 时自动：`cargo check` → `cargo build` → 上传 artifact
→ 刷新 `latest` 发布。推 `v*` 形式的 tag 会额外建一个带版本号的 Release。

### ⚠️ 为什么推送必须用 SSH

这台机器的网络实测结果：

| 端点 | 结果 |
|---|---|
| `github.com:443` | ❌ TCP 超时（被阻断，HTTPS 推送走这里，所以必定失败） |
| `github.com:22` | ✅ 通（约 1 秒） |
| `ssh.github.com:443` | ✅ 通 |
| `api.github.com:443` | ✅ 通 |

本机有 Clash 代理在 `127.0.0.1:7897`，但 git 没配置走它。因此**远程地址必须是 SSH**：

```powershell
git remote set-url origin git@github.com:ismoneky/proc-cleaner.git
git push
```

用 HTTPS 地址会得到：

```
fatal: unable to access 'https://github.com/...': Failed to connect to github.com:443
```

如果哪天想改回 HTTPS，得先让 git 走代理：

```powershell
git config --global http.proxy http://127.0.0.1:7897
```

（或者在 Clash 里开启 TUN 模式。）

### 本机打包（可选）

`build.ps1` 依然可用，但要先装 Rust 工具链。本机没装，所以走云端。

---

## 八、构建状态（截至本次交付）

| 项 | 状态 |
|---|---|
| `cargo check --release` | ✅ 云端通过 |
| `cargo build --release` | ✅ 云端通过 |
| 产物 | `proc-cleaner.exe`，4.52 MB |
| PE 校验 | `MZ` + `PE\0\0`，machine = `0x8664`（x86_64） |
| SHA256 | `EE64A622771854E19BD7FFFDBC9AC2CCD6CD0CDF02B157833683922FF943660A` |
| 界面实际显示 | ⚠️ **未由我验证**（非交互会话里 GUI 启动测试不稳定，请自行双击确认） |
| 结束进程的实际效果 | ⚠️ **未验证** |

原先 README 第四节列的「未验证清单」现在可以更新：编译、`windows::core::BOOL`
路径、以及 `egui 0.31` + `sysinfo 0.33` 的 API 组合，**都已由云端构建证实**。

剩下真正没验证的只有两条：**界面长什么样**，和**杀进程是否真的生效**（后者需要
管理员权限，且会真的结束进程，不适合自动测试）。

