---
description: 改 AGENTS.md 或 .omp/** 时的分层归属、维护约定与同改矩阵（新知识该落在哪一层）
condition: '[\s\S]'
scope: "tool:edit, tool:write"
globs:
  - '**/AGENTS.md'
  - '**/.omp/**'
interruptMode: never
---

你正在改 AI 工作契约（`AGENTS.md` / `.omp/**`）。先判定内容该落在哪一层，再按同改矩阵把配套面一次改完。

## 分层归属（按「何时需要在上下文里」选层，宁低勿高）

|层|载体|进上下文的时机|只放什么|
|---|---|---|---|
|粘性红线|`.omp/RULES.md`|每次请求（长会话也不掉）|不可逆操作门槛、分支模型、用户明确拍板的产品决定（写日期）。保持 ≤ 3 KB|
|核心契约|`AGENTS.md`|会话开头一次|架构缝、跨域硬不变式（每条一两行）、命令、单一事实源坐标、各层索引。保持约 20 KB；新增一条前先看能否下沉到路径规则或附录|
|路径规则|`.omp/rules/*.md`（`condition: '[\s\S]'` + `scope: "tool:edit, tool:write"` + `globs: ['**/路径']` + `interruptMode: never`）|编辑命中路径时注入一次|只在改某处时才需要的约束：镜像契约（`mirror-*`）、领域细则、补丁陷阱|
|流式拦截规则|`.omp/rules/*.md`（`condition` 正则 / `astCondition`，可打断）|输出命中模式时|机械可判的违规写法（unwrap、unsafe、禁用命令）|
|执行前拦截|`.omp/hooks/pre/guard.ts`|工具调用前|不可绕过的命令红线|
|知识附录|`.omp/knowledge/*.md`|按需 `read`|枚举细节、子系统说明、踩坑（`pitfalls.md`，带代码坐标）|

- 不依赖 omp memory 后端存项目知识：它在本机 agent 目录、不随仓分发（当前 `memory.backend` 为 off）。项目知识一律进上面这几层，随仓分发、可审查。
- 新知识归属：用户拍板的产品 / 交互决定 → `RULES.md`（带日期）；有代码坐标的坑 → `pitfalls.md` 对应领域小节；跨端「改 A 必须改 B」→ 对应 `mirror-*` 规则（没有合适的就按路径新建一条）；其余事实 → 对应 knowledge 附录。
- 路径规则的 `globs` 一律写 `'**/<仓库相对路径>'`（相对 / 绝对路径都能命中）；只写具体路径不带通配符的 `condition` 会被当成正则匹配内容，**不会**按路径触发。兜底条件用 `'[\s\S]'` 而不是 `'.*'`。新增或改 glob 后用 `omp ttsr test --rule <规则> --source tool --tool edit --path <文件> 'x'` 正反例各验一次。

## 维护约定

- 只有**架构 / 契约 / 不变式**变化才改 `AGENTS.md`；改一个普通文件不必回来更新它。
- 任何文档都**不**维护「完整文件清单 / 完整设置项清单」这类每次提交都漂移的枚举——源码是唯一事实源（`read <dir>` 看结构，`grep` 查事实）。
- 事实层（版本号、协议数、路由、env、DB 列、设置键）以代码为准，文档只给坐标。
- **知识文档只能放 `.omp/`**：`docs/` 与 `.agents/` 都在 `.gitignore` 里（本地目录，零文件入库、不随仓分发）；`docs/*.md` 只是本机设计草稿，别把契约写进去。

## 同改矩阵

|改了什么|同回合必须过一遍|
|---|---|
|架构缝 / 不变式（`AGENTS.md` §3–§4）|`.omp/knowledge/*` 对应附录；需要编辑期提醒的落路径规则，需要执行期拦截的进 `guard.ts`|
|镜像契约（`mirror-*` 规则）|规则的 `globs` 覆盖两侧所有文件；`AGENTS.md` §5 索引表同步增删|
|红线 / 授权 / 禁用命令|`.omp/RULES.md`、`.omp/rules/forbidden-commands.md`（TTSR 提示）、`.omp/hooks/pre/guard.ts`（执行前硬拦截：tag 推送（CI + 同名演练全绿自动放行，否则确认）、`--no-verify`、强推、测试 `--workspace`、子代理全量门禁）、`.omp/WATCHDOG.md`（严重级映射）|
|CI 触发路径（`ci.yml` 的 `changes.filters`）、新增 crate / path 依赖 / `include_*!` / TS 跨目录引用|`python3 .github/scripts/check_ci_paths.py` 必须通过（CI `ci-paths` job 同跑）|
|`AGENTS.md` 章节编号 / 标题（`guard.ts`、skills、rules、knowledge 都按 `§N` 引用）|`grep "AGENTS.md"` 扫 `.omp/`、源码注释，修掉悬空引用|
