---
title: 安装
description: 在 Windows、macOS 或 Linux 上安装 FluxDown,完成首次启动配置。
section: getting-started
order: 1
sourceHash: "51af74b174e5"
---

FluxDown 为 Windows、macOS、Linux 三大平台提供完整的原生构建,每个安装包内置相同的 Rust 下载引擎与相同的界面——没有"精简版",也不需要注册账号。

## 系统要求

| 平台 | 要求 |
|---|---|
| Windows | Windows 10(64 位)或更高版本,支持 x64 / ARM64 |
| macOS | macOS 10.15(Catalina)或更高版本,支持 Apple Silicon / Intel |
| Linux | 64 位、具备现代 GTK3 环境的桌面发行版 |

## Windows

从[下载页](/#download)获取安装包,x64 和 ARM64 均提供以下两种形式:

- **安装版** —— `FluxDown-<版本号>-setup.exe`。标准 Inno Setup 安装向导,仅为当前用户安装到 `%LOCALAPPDATA%\Programs\FluxDown`,安装、更新都无需管理员权限;浏览器扩展(Native Messaging)、链接协议、`.torrent` 关联、开机自启也都只写当前用户的注册表。安装过程中可勾选创建桌面快捷方式、开机自动启动、将 `.torrent` 文件关联到 FluxDown——三项默认均不勾选。旧版本若曾选择「为所有用户安装」(装在 `Program Files`),新安装程序会提示并请求**一次**管理员权限卸载旧副本,已开启的自启、关联与链接协议会自动改指新位置,下载任务与设置保留。
- **便携版** —— `FluxDown-<版本号>-windows-<架构>-portable.zip`。解压到任意目录后运行 `fluxdown-desktop.exe` 即可;请保持所有文件在同一目录——桌面程序会拉起同目录的 `fluxdown-agent.exe` 与 `fluxdownd.exe`。除首次启动时你主动选择的项目外,不会在解压目录之外写入任何内容。

安装包未做代码签名,首次运行时 Windows SmartScreen 可能提示"未知发布者"。点击**更多信息 → 仍要运行**即可继续。

### Scoop

如果你使用 [Scoop](https://scoop.sh) 包管理器,FluxDown 已收录进官方 Extras 源,一条命令安装:

```powershell
scoop bucket add extras
scoop install extras/fluxdown
```

此方式安装便携版,并在升级时保留你的 `settings.json`。新版本会自动进入 Extras,随时用 `scoop update fluxdown` 更新。

## macOS

- **DMG** —— `FluxDown-<版本号>-macos-<架构>.dmg`(Apple Silicon 选 `arm64`,Intel 选 `x64`)。打开后将 FluxDown 拖入**应用程序**文件夹。
- **便携版** —— `FluxDown-<版本号>-macos-<架构>.tar.gz`。解压后直接运行其中的 App。

两种形式都已使用 Apple Developer ID 签名并经 Apple 公证,打开时不会出现 Gatekeeper 拦截提示。

## Linux

Linux 全部为 x64 架构,提供四种形式:

- **AppImage** —— `FluxDown-<版本号>-linux-x64.AppImage`。赋予可执行权限(`chmod +x`)后直接运行。近几年发布的发行版可能需要安装 `libfuse2` 才能运行 AppImage。
- **deb** —— `FluxDown-<版本号>-linux-x64.deb`,适用于 Debian/Ubuntu 及其衍生版:`sudo apt install ./FluxDown-<版本号>-linux-x64.deb`。
- **Arch 软件包** —— `FluxDown-<版本号>-linux-x64.pkg.tar.zst`:`sudo pacman -U FluxDown-<版本号>-linux-x64.pkg.tar.zst`。
- **便携版** —— `FluxDown-<版本号>-linux-x64.tar.gz`。解压后运行其中的可执行文件。

## 首次启动

FluxDown 首次启动时会静默完成系统层面的接入配置,让其他应用发来的链接和文件能直接交给它处理:

- **`fluxdown://` 协议** —— 每次启动都会自动注册,供浏览器扩展等应用向 FluxDown 传递下载请求。
- **Native Messaging Host** —— 自动注册,供浏览器扩展通过 Native Messaging(Windows 下是命名管道,Linux/macOS 下是 Unix socket)与 FluxDown 通信。
- **`.torrent` 文件关联** —— **不会**自动完成。如果安装时没有勾选关联选项,FluxDown 会在首次启动时弹出一次性对话框询问是否设为默认 `.torrent` 打开方式。你可以接受、忽略,或之后随时在**设置 → 通用 → 关联 .torrent 文件**中修改。

## 自动更新

启动 5 秒后,FluxDown 会在后台静默检查 GitHub Releases 上是否有新版本(可在**设置 → 关于 → 自动检查更新**中关闭)。发现新版本时会弹出更新日志对话框,提供**立即更新**与**稍后提醒**两个选项。你也可以随时在**设置 → 关于**或侧边栏底部的版本号处手动检查。

## 卸载

- **Windows(安装版)** —— 打开**设置 → 应用 → 已安装的应用**,找到 FluxDown 卸载即可;便携版直接删除解压目录。
- **macOS** —— 将 FluxDown 从**应用程序**拖入废纸篓。
- **Linux** —— `sudo apt remove fluxdown`(deb)、`sudo pacman -R fluxdown`(Arch),或删除 AppImage / 解压目录。

卸载只会移除应用本身,已下载的文件会留在你保存的位置,不会被一并删除。
