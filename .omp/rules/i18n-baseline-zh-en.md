---
description: FluxDown 多语言以英文 + 简体中文为开发基线；assets/i18n 的其余语言（zh-TW 等）由 ZerxLabBot 翻译自动化维护，开发中只补基线
condition:
  - 'assets/i18n/*.json'
  - 'web/src/i18n/*'
  - 'website-v2/src/i18n/messages/*'
  - 'fluxDown/utils/locales/*'
interruptMode: never
---

你正在改 FluxDown 的翻译文件。**开发中只维护 en + zh 两条基线**；`assets/i18n/` 下的其余语言（目前 `zh-TW`）由 ZerxLabBot 的 i18n 自动化补齐（见下文「非基线语言」），开发者新增/改文案时**不必、也不要**顺手补它们。

## 三个 i18n 文件面各自的基线对（缺一不可）

| 面 | 基线文件 | 备注 |
|---|---|---|
| GPUI（`crates/*`）+ Web SPA（`web/`）+ 原生 Android/iOS（`mobile/`） | `assets/i18n/en.json` + `assets/i18n/zh.json` | 四端共用同一 camelCase 键：GPUI `ctx.t("key")`；Web `useT()`；Android 构建期生成 `R.string`；iOS `L("key")`。删键前检查 `crates/`、`web/src/` 与 `mobile/` 的消费者 |
| 官网（`website-v2/`） | `website-v2/src/i18n/messages/<ns>.ts` | 每个模块用 `defineMessages({ en, zh })` 同时声明两语，zh 结构必须与 en 完全一致，缺键/多键 `astro check` 直接报错；只有 en + zh，无社区语言 |
| 浏览器扩展 | `fluxDown/utils/locales/zh-CN.ts` + `en.ts` + `zh-TW.ts` | 反过来：`MessageKey` 类型由 **`zh-CN.ts`** 推导，`en.ts` 与 `zh-TW.ts` 都受 `Record<MessageKey, string>` 约束，少一个键直接 tsc 报错，所以扩展新增键三份一起补；扩展不在翻译自动化范围内 |

官网文档另算：`website-v2/src/content/docs/{en,zh}/`，改了 en 正文要在 `website-v2/` 下跑 `npm run docs:hash <zh 文件>` 刷新 zh 的 `sourceHash`；en-only 页会自动回退，不必为其造 zh 占位。

## 加/改一个 UI 字符串的完整动作

1. `en.json` 与 `zh.json`（官网为同一 `defineMessages` 的 `en`/`zh` 两支；扩展为 `zh-CN.ts` + `en.ts` + `zh-TW.ts`）**同名键、同时补上、都非空**。
2. 各端通过已有 i18n 查表/生成入口消费共享键，参数化文案使用 `{name}` 占位；不要建立平行翻译表。
3. 删键同理两边一起删；改键名等于删旧加新。
4. **不要手补 `assets/i18n/` 的非基线语言**：缺键在运行时按键级回退英文，不是 bug；自动化每天发现并开 PR 补齐。人工修正某条已有译文可以直接改（自动化只重译缺失键与 en 原文变化的键，不覆盖人工修正）；同类术语问题优先改 `i18n/glossary/<locale>.md`。
5. GPUI（`build.rs`）、Web（Vite 插件 / bun 预加载生成 `virtual:i18n-tables`，见 `web/i18n-tables.ts`）、Android（`GenerateI18nResources` + `localeFilters`）与 iOS 运行时都自动发现 `assets/i18n/*.json`；唯一手动项是 iOS 的 `mobile/FluxDown/Config/Info.plist` `CFBundleLocalizations` 与工程 `knownRegions`（否则系统 App 语言设置不列出），`mobile/FluxDown/scripts/check-i18n.py` 会报未声明的语言。新增一门语言 = 加 JSON（至少含 `languageNativeName`）+ iOS 两处声明 + `i18n/glossary/<locale>.md`，之后自动化负责补全。官网 v2 的语言集固定在 `website-v2/src/i18n/config.ts::LANGS`（en + zh），不做翻译。
6. 原生移动端：Android 构建期由共享 JSON 生成 `R.string.<键>`，**禁止**在 `res/` 手写同名文案；iOS 经 FluxKit 符号链接打包同一 JSON、`L("键")` 查表，`mobile/FluxDown/scripts/check-i18n.py` 校验漏键。

## 非基线语言（自动化维护）

- 范围：`assets/i18n/` 下除 `en.json` / `zh.json` 外的语言文件（GPUI / Web / Android / iOS 共用）。官网与扩展不在范围内。
- 流程：服务器 OpenHands 自动化 `i18n`（ZerxLabBot）每日扫描缺失 / 过期 / 孤儿键 → 按 `i18n/glossary/<locale>.md` 翻译 → 校验占位符与键集 → 滚动 PR `bot/i18n-sync`，**维护者手动合并**。
- 过期判定：`i18n/source-lock.json` 按语言、按键记录译文对应的 en 原文哈希；en 原文一改即重译。该文件只由自动化写，不要手改。
- 兜底：`crates/i18n` 单测 `translated_locales_keep_baseline_placeholders` 要求非基线语言的占位符不超出 en ∪ zh、不丢 en ∩ zh 都有的占位符，人工改译文同样受约束。

## 自检（在 FluxDown 仓根执行）

```bash
node -e "const a=require('./assets/i18n/en.json'),b=require('./assets/i18n/zh.json');const A=Object.keys(a),B=Object.keys(b);console.log('zh 缺/空:',A.filter(k=>!(k in b)||!b[k]));console.log('zh 多余:',B.filter(k=>!(k in a)))"
```

上面的脚本只适用于 `assets/i18n` 基线对：官网由 `cd website-v2 && bun run check`（`astro check`）把关；扩展侧由 `tsc` 把关，跑 `cd fluxDown && npx tsc --noEmit` 或直接看 IDE 报错。非基线语言的缺键不用检查。
