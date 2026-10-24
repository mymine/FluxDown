---
description: 四端 UI 展示用户/主机数据（路径、文件名、URL、设备名、数字）时，必须按真实数据形态与平台差异自查边界情况
condition:
  - 'crates/*/src/**/*.rs'
  - 'web/src/**/*.tsx'
  - 'mobile/Android/app/src/main/**/*.kt'
  - 'mobile/Android/fluxui/src/main/**/*.kt'
  - 'mobile/FluxDown/FluxDown/**/*.swift'
interruptMode: never
---

你正在改 FluxDown 的界面。设计稿 / 截图里的样例数据（`/Users/zero/Downloads/Video`、短英文文件名）**不代表真实数据**。凡是把用户或主机提供的值渲染出来，交付前逐项自问；只验证「我这台 macOS 上的样例」不算完成。

## 数据形态清单

|数据|必须考虑|
|---|---|
|文件系统路径|深层长路径；Windows 盘符 `C:\`、`\` 分隔、UNC `\\nas\share`、末尾分隔符；空格 / 连续空格 / 中日韩 / emoji / 组合字符；Android `/storage/emulated/0/…`、`/sdcard`、SD 卡 `/storage/XXXX-XXXX`、应用私有目录；iOS 沙盒 `/var/mobile/Containers/Data/Application/<UUID>/…`|
|文件名 / 任务名|超长名、无扩展名、多段扩展名（`.tar.gz`、`.7z.001`）、百分号编码、非 UTF-8 来源、RTL 字符|
|URL|超长查询串、`magnet:` / `ed2k://` / `torrent-file://` 哨兵、IDN|
|名称类（设备 / 分类 / 队列 / 插件）|空、超长、同名、本地化后长度（德/俄文更长）|
|数字|0、未知、极大值（>2 GB 字节、>1 万任务）、负值/哨兵|

## 显示规则（四端一致）

- **路径单行中间省略**：保留开头（盘符 / 卷）与末段目录名，悬停 / 长按可见完整值；GPUI `text_ellipsis_middle`、Web `ui/PathText.tsx`、Android `TextOverflow.MiddleEllipsis`、iOS `.truncationMode(.middle)`。文件名同理（保留扩展名）。
- **弹出层 / 菜单必须限宽限高**，内容不得把菜单撑出窗口；列表项文本列要 `min-w-0` / `weight(1f)` 才能真正截断。
- **本机路径友好显示**：Android 本机目录一律经 `ui/LocalStorage.kt::friendlyLocalPath`（「内部存储 › …」）；**远端主机路径原样显示**（它可能是 Windows / NAS 路径，不能套本机卷名）。仅显示层转换，保存 / 比较 / 提交仍用真实路径。
- 保留连续空格（Web 用 `white-space: pre`，禁止依赖 HTML 空白折叠）；截断只用渲染层省略或按字符边界，禁止按字节切。
- 路径比较 / 判等只做 `trim`，不做大小写或分隔符归一（与 `CategoryRouteDto::follows` 一致）；需要归一时改契约并同步四端。

## 交付前自检

1. 用上表至少三种「坏数据」走一遍新 UI 的每个文本位（输入框、菜单项、触发器、提示、通知）。
2. 纯展示逻辑（拆分 / 格式化 / 友好名）写成纯函数并用这些样例做单测（参照 `web/src/lib/pathDisplay.test.ts`、Android `LocalDirectoryFilesTest.friendlyPath…`）。
3. 无法目视的平台在最终回复里写明「未做 GUI 验证」与具体检查样例（如「把分类目录设为 `D:\下载 2024\  深层\…\Programs` 看菜单」）。
4. 新发现的平台显示坑，同回合补进 `.omp/knowledge/pitfalls.md` 对应小节（带坐标）。
