# YouTube 下载插件（fluxdown@youtube-down）

经 FluxDown 的 yt-dlp 组件解析 YouTube，把直链交回引擎做多段下载、断点续传与音视频合并；同时向浏览器扩展贡献 YouTube 嗅探规则，让播放器上直接出现下载浮钮。纯 JS，无需构建，直接以 dev 目录安装。

## 依赖

- App「组件」页安装 **yt-dlp**；分离的音视频轨合并需要 **ffmpeg**。
- JavaScript 运行时（Deno、Node.js、QuickJS 或 Bun 任一），且在终端里能直接运行：yt-dlp 需要它求解 YouTube 的签名挑战。App 调用 yt-dlp 时按登录 shell（macOS / Linux）或注册表（Windows）里的完整 PATH 查找，从 Dock / 开始菜单启动也一样。
- 与索引仓的 `fluxdown@ytdlp` 插件同时启用时两者都匹配 YouTube 链接，只保留一个。

## 安装

设置 → 扩展 → 插件 → 「安装开发目录」，选择本目录 `packages/plugin-sdk/examples/youtube-down`，授权 `ytdlp` 与 `browserRules`。

## 支持的链接

|链接|处理|
|---|---|
|`youtube.com/watch?v=…`（含 `m.` / `music.`）、`youtu.be/…`、`/shorts/…`、`/live/…`、`/embed/…`、`youtube-nocookie.com/embed/…`|单个视频。带 `list=` 的播放页只下当前视频|
|`youtube.com/playlist?list=…`|播放列表：返回清单，按条目裂变子任务（条数上限见设置）|

直播进行中与预告不可下载；直播结束、YouTube 处理完成后可下载回放。

## 清晰度与格式

- 只使用可按 HTTP Range 直接下载的格式（排除 HLS、故事板、DRM 与动态范围压缩音轨）。
- 每个清晰度挑一个最佳视频轨，配同容器的音频轨：MP4 视频配 M4A（AAC），WebM 视频配 WebM（Opus）；容器对不上时落成 MKV。多音轨视频优先原声。
- 「清晰度」设为「每次询问」时，新建任务弹框列出各清晰度与「仅音频」，60 秒未选按最高画质；其余选项直接下载不超过该高度的最高档。播放列表条目从不弹框，规格取清单条目的首个规格（即设置里的清晰度，「每次询问」视为最高画质）。

## 浏览器扩展规则（`rules.json`）

插件启用且已授权时，规则随 plugin 层同步到扩展（id 改写为 `plugin.fluxdown.youtube-down.*`）：

|规则|作用|
|---|---|
|`ignore-googlevideo`|YouTube 页面上 `googlevideo.com` 的播放分片不再作为下载资源（它们是带 Range 的流式片段，单独下载没有意义）|
|`watch-page`|播放页 / Shorts / 直播页上，主播放器的 `<video>` 一挂上媒体源，就把当前页面 URL 作为该播放器的视频资源（`${page}` 模板字段），并归属这个 `<video>`：浮钮与「下载正在播放的媒体」快捷键直接可用，下载请求带着浏览器里的 YouTube Cookie 交给桌面端，由本插件解析|
|`watch-title`|候选名取播放页标题 `ytd-watch-metadata h1`|

## 验收清单

- [ ] 安装后授权对话框列出 `ytdlp`、`browserRules` 两个权限，授权后插件已加载、已启用。
- [ ] App 新建下载粘贴 `https://youtu.be/<id>`：创建任务后弹清晰度框（含 2160p…360p 与「仅音频」），选择后下载、合并为单个文件，文件名为视频标题。
- [ ] 设置「清晰度」= ≤ 720p：同一链接不再弹框，直接下 720p。
- [ ] 粘贴 `https://www.youtube.com/playlist?list=<id>`：新建框预览出条目清单，每条可选规格；确认后按条目建子任务并下载。
- [ ] 浏览器扩展选项页 → 规则：plugin 层出现上面三条规则；禁用插件后再同步，规则清空。
- [ ] 打开 YouTube 播放页：浮钮出现在播放器上（暂停时也在），点击下载交给桌面端；面板 / 弹窗里只有这一个视频候选，没有 `googlevideo.com` 分片。
- [ ] 站内点击另一个视频（SPA 换页）：候选换成新视频，旧视频不残留。
- [ ] Shorts 页上下滑动：浮钮对应当前 Short。
- [ ] 未登录 / 被风控时任务报错文案提示从浏览器发起下载或填写备用 Cookie；从已登录 YouTube 的浏览器发起的下载可以通过。
