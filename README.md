# OpenNote

OpenNote is a local-first Markdown note app for Windows. Your notes are plain
Markdown files in a folder you choose (a "vault"). Nothing is uploaded: the app
has no account, no cloud service and no telemetry.

[简体中文](#简体中文)

## Download and install

1. Open the [Releases](../../releases) page and download
   `OpenNote_<version>_x64-setup.exe`.
2. Double-click it and follow the setup wizard. No administrator rights are
   needed; OpenNote installs for your user account only.
3. Start OpenNote from the Start menu or the desktop shortcut.

> The installer is not code-signed yet, so Windows SmartScreen may say
> "Windows protected your PC". Click **More info → Run anyway**.

OpenNote needs the Microsoft WebView2 runtime. It is built into Windows 11;
on Windows 10 the installer downloads it automatically if it is missing.

**Uninstall:** Settings → Apps → Installed apps → OpenNote → Uninstall. Tick
"Delete the application data" to also remove settings and caches. Your notes
are never deleted: they stay in your vault folder.

## Features

- Markdown editing with live, source and reading modes; find and replace
- Vaults with a file tree, tabs, internal links, backlinks, tags and properties
- Full-text search, tasks, daily notes and templates
- Images and attachments, an in-vault PDF reader
- Canvas and database views
- LaTeX projects: templates, compile, preview, diagnostics, export to ZIP or PDF
- Import, export (HTML, PDF via print), trash, local history and local backup
- Customisable shortcuts, light/dark themes, 16 interface languages
  (translations are machine-assisted and not yet reviewed by native speakers)

## Building from source

For contributors only; regular users should use the installer above.

Requirements: Windows 10/11, [Rust](https://rustup.rs/) 1.89+ with the MSVC
toolchain (Visual Studio Build Tools), Node.js 20+.

```powershell
npm install
npm run tauri -- build
```

The installer is written to `target\release\bundle\nsis\`.

Tests:

```powershell
npm run typecheck
npm run test
cargo test --workspace
```

| Path | Contents |
| --- | --- |
| `src/` | React frontend (editor, workspace, LaTeX workspace, i18n) |
| `src-tauri/` | Tauri desktop host, IPC commands, installer configuration |
| `crates/` | Rust libraries: vault storage, search, links, tasks, history/backup, LaTeX, diagnostics |
| `public/` | Static images used by the app |
| `scripts/` | Build helpers |

## License

[MIT](LICENSE)

---

## 简体中文

OpenNote 是一个本地优先的 Windows Markdown 笔记应用。笔记就是你选定文件夹（"库"）
里的普通 Markdown 文件，不上传任何内容：没有账号、没有云服务、没有遥测。

### 下载安装

1. 打开 [Releases](../../releases) 页面，下载 `OpenNote_<版本>_x64-setup.exe`。
2. 双击运行，按安装向导点"下一步"即可。不需要管理员权限，只为当前用户安装。
3. 从开始菜单或桌面快捷方式打开 OpenNote。

> 安装包暂未做代码签名，Windows 可能提示"Windows 已保护你的电脑"，点 **更多信息 → 仍要运行**。

需要 WebView2 运行时：Windows 11 自带；Windows 10 若缺少，安装程序会自动下载。

**卸载：** 设置 → 应用 → 已安装的应用 → OpenNote → 卸载。勾选"删除应用程序数据"可一并删除设置和缓存。
你的笔记不会被删除，它们一直在你的库文件夹里。

### 功能

实时/源码/阅读三种编辑模式、查找替换、文件树与标签页、内部链接与反向链接、标签与属性、
全文搜索、任务、日记与模板、图片附件、库内 PDF 阅读、画布与数据库视图、LaTeX 项目
（模板、编译、预览、诊断、导出 ZIP/PDF）、导入导出、回收站、本地历史与备份、可改快捷键、
深浅色主题、16 种界面语言（翻译未经母语者审校）。

### 从源码构建（仅开发者）

需要 Rust 1.89+（MSVC）和 Node.js 20+，执行 `npm install` 后运行 `npm run tauri -- build`，
安装包输出在 `target\release\bundle\nsis\`。

许可证：[MIT](LICENSE)
