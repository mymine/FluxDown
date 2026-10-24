/**
 * FluxDown 命令守卫：把 .omp/RULES.md（git 授权 / 分支模型）与 AGENTS.md §2、§7「并行子代理」里机械可判定的红线在工具调用前硬拦截。
 *
 * TTSR 规则（.omp/rules/forbidden-commands.md）只在输出流里打断并提示，模型可原样重试；
 * 这里在执行前直接拒绝，不可绕过。规则正文仍以 RULES.md / AGENTS.md 为准，本文件只做判定。
 *
 * - 推送 v* tag / --tags / --follow-tags / --mirror：触发不可逆的全平台发布，交互会话弹确认，无 UI 时拒绝。
 * - --no-verify：绕过 .githooks 本地门禁（AGENTS.md §2 明令禁止）。
 * - 裸 --force 推送：只允许 --force-with-lease；main / stable 一律禁止强推。
 * - 推送 stable 前校验分支模型一致性：`git log stable --not main` 必须为空。
 * - `cargo test|nextest --workspace`：测试禁全量（§2）。
 * - 子代理（build-verifier 除外）禁止全量门禁与移动端整包构建：多个子代理共享 target/ 与机器资源，
 *   全量 clippy / 无 -p 的 cargo 构建测试、gradlew、xcodebuild、build-core.sh 由主会话或 build-verifier 跑一次。
 */
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import type { ExtensionAPI } from "@oh-my-pi/pi-coding-agent";

export type Verdict = { block: true; reason: string } | { confirm: string } | undefined;

export interface Caller {
	/** 子代理（task / eval agent / 克隆会话 / advisor）。 */
	isSub: boolean;
	/** 小写的子代理定义名，例如 `build-verifier`。 */
	name: string;
}

const RELEASE_TAG = /^(?:\+?)(?:refs\/tags\/)?v\d/;
const PROTECTED = /^(?:\+)?(?:refs\/heads\/)?(?:main|stable)$/;
const CARGO_HEAVY = /\bcargo\s+(?:\+\S+\s+)?(build|b|test|t|nextest|clippy|run|r|bench|doc)\b/;
const MOBILE_BUILD = /(?:^|[\s/])(?:gradlew|xcodebuild|build-core\.sh|testflight\.sh|sideload\.sh|package\.sh)\b/;

/** 引号内的词：带此前缀后永远不会被当作选项或 refspec 识别。 */
const QUOTED = "\u0000";

/**
 * 拆成简单命令的词列表（策略判定足够，不是完整 shell 解析器）：识别单双引号与反斜杠转义，
 * 只在引号外按 `&&` `||` `;` `|` 换行切分命令；引号内的词加 QUOTED 前缀。
 */
function commands(line: string): string[][] {
	const out: string[][] = [];
	let words: string[] = [];
	let word = "";
	let quoted = false;
	let quote: string | undefined;
	const endWord = () => {
		if (word || quoted) words.push(quoted ? QUOTED + word : word);
		word = "";
		quoted = false;
	};
	const endCommand = () => {
		endWord();
		if (words.length) out.push(words);
		words = [];
	};
	for (let i = 0; i < line.length; i++) {
		const c = line[i]!;
		if (quote) {
			if (c === quote) quote = undefined;
			else if (c === "\\" && quote === '"' && i + 1 < line.length) word += line[++i];
			else word += c;
		} else if (c === "'" || c === '"') {
			quote = c;
			quoted = true;
		} else if (c === "\\" && i + 1 < line.length) {
			word += line[++i];
		} else if (c === ";" || c === "\n" || c === "|" || c === "&") {
			endCommand();
		} else if (/\s/.test(c)) {
			endWord();
		} else {
			word += c;
		}
	}
	endCommand();
	return out;
}

/** 带值的 git 选项：其后一个词是值，不参与选项判定。 */
const GIT_VALUE_OPTS: Record<string, true> = {
	"-m": true,
	"--message": true,
	"-F": true,
	"--file": true,
	"--author": true,
	"--date": true,
	"-C": true,
	"-c": true,
	"-t": true,
	"--template": true,
};

/** 取 git 子命令及其参数，跳过 `-C dir` / `-c k=v` 等全局选项；不是 git 命令返回 undefined。 */
function gitArgs(words: string[]): { args: string[]; dir?: string } | undefined {
	// 只认命令位置上的 git（前面只允许环境变量赋值与 env / command / sudo），`echo git push` 不算。
	const at = words.findIndex(w => !/^[A-Za-z_][A-Za-z0-9_]*=/.test(w) && !["env", "command", "sudo"].includes(w));
	if (at < 0 || !(words[at] === "git" || words[at]!.endsWith("/git"))) return undefined;
	let dir: string | undefined;
	let i = at + 1;
	while (i < words.length && words[i]!.startsWith("-")) {
		if (words[i] === "-C") dir = words[i + 1]?.replace(QUOTED, "");
		i += words[i] === "-C" || words[i] === "-c" ? 2 : 1;
	}
	const [sub, ...rest] = words.slice(i);
	// 选项值（提交信息等）替换成引号词，避免 `-m -n` 之类被误判。
	const args = rest.map((w, k) => (k > 0 && GIT_VALUE_OPTS[rest[k - 1]!] ? QUOTED + w : w));
	return { args: sub === undefined ? [] : [sub, ...args], dir };
}

/** refspec 的目标端：`src:dst` 取 dst，否则即自身。 */
function destination(refspec: string): string {
	const colon = refspec.lastIndexOf(":");
	return colon >= 0 ? refspec.slice(colon + 1) : refspec;
}

function checkPush(rest: string[], stableDiverged: () => string | undefined): Verdict {
	const flags = rest.filter(a => a.startsWith("-"));
	// 位置参数：首个是远端，其余是 refspec（引号内的 refspec 照常判定）。
	const specs = rest
		.filter(a => !a.startsWith("-"))
		.slice(1)
		.map(a => a.replace(QUOTED, ""));
	const refs = specs.map(destination);
	const lease = flags.some(a => a.startsWith("--force-with-lease"));
	const force = flags.some(a => a === "--force" || a === "-f" || /^-[a-zA-Z]*f[a-zA-Z]*$/.test(a));
	const plusForce = specs.some(r => r.startsWith("+"));
	// `git push origin "$V"` 这类变量 refspec 无法在执行前解析，按可能是发布 tag 处理。
	const unresolved = refs.some(r => /[$`]/.test(r));

	if ((force || plusForce || lease) && refs.some(r => PROTECTED.test(r))) {
		return { block: true, reason: "禁止强推 main / stable（.omp/RULES.md 分支模型）。" };
	}
	if ((force || plusForce) && !lease) {
		return { block: true, reason: "只允许 --force-with-lease，禁止裸 --force / +refspec 强推。" };
	}
	if (
		flags.some(a => a === "--tags" || a === "--follow-tags" || a === "--mirror") ||
		refs.some(r => RELEASE_TAG.test(r)) ||
		unresolved
	) {
		return {
			confirm:
				"推送 v* tag 会立即触发全平台发布流水线，不可逆（.omp/RULES.md）。只有用户在本会话明确要求发版时才允许。",
		};
	}
	if (refs.some(r => /^(?:refs\/heads\/)?stable$/.test(r))) {
		const diverged = stableDiverged();
		if (diverged) {
			return {
				block: true,
				reason: `stable 含有 main 没有的提交（git log stable --not main 非空），先同步回 main 再推送：\n${diverged}`,
			};
		}
	}
	return undefined;
}

/** 判定一条 bash 命令。`stableDiverged(dir)` 返回该仓库 `git log stable --not main --oneline` 的非空输出。 */
export function checkBash(
	line: string,
	caller: Caller,
	stableDiverged: (dir?: string) => string | undefined = () => undefined,
): Verdict {
	for (const words of commands(line)) {
		const git = gitArgs(words);
		if (git) {
			const [sub, ...rest] = git.args;
			const noVerify = rest.some(a => a === "--no-verify") || (sub === "commit" && rest.some(a => /^-[a-zA-Z]*n[a-zA-Z]*$/.test(a)));
			if (noVerify && ["commit", "merge", "push", "rebase", "cherry-pick", "am", "revert"].includes(sub ?? "")) {
				return {
					block: true,
					reason: "禁止 --no-verify 绕过本地 hook（AGENTS.md §2）。修复 hook 报告的问题（如 cargo fmt 后重新 add）再提交。",
				};
			}
			if (sub === "push") {
				const verdict = checkPush(rest, () => stableDiverged(git.dir));
				if (verdict) return verdict;
			}
			continue;
		}

		// 只看引号外的词：`echo "cargo test --workspace"` 之类不是在执行 cargo。
		const cmd = words.filter(w => !w.startsWith(QUOTED)).join(" ");
		const cargo = CARGO_HEAVY.exec(cmd);
		if (cargo && /\s--workspace\b/.test(cmd) && /^(?:test|t|nextest)$/.test(cargo[1]!)) {
			return {
				block: true,
				reason: "测试禁止 --workspace（AGENTS.md §2）：用 `cargo nextest run -p <crate> <filter>` 精准跑。",
			};
		}

		if (caller.isSub && caller.name !== "build-verifier") {
			const wholeWorkspace = cargo && (/\s--workspace\b/.test(cmd) || !/\s(?:-p|--package)[\s=]/.test(cmd));
			if (wholeWorkspace || MOBILE_BUILD.test(cmd)) {
				return {
					block: true,
					reason:
						"子代理只跑自己改动 crate 的精准命令（`cargo check|clippy|nextest run -p <crate>`）。" +
						"全量 clippy、无 -p 的 cargo 构建/测试与 gradlew / xcodebuild / build-core.sh 由主会话或 build-verifier 统一跑一次；" +
						"在报告里写明需要跑哪些命令。",
				};
			}
		}
	}
	return undefined;
}

function stableDivergence(cwd: string): string | undefined {
	try {
		const out = execFileSync("git", ["-C", cwd, "log", "stable", "--not", "main", "--oneline"], {
			encoding: "utf8",
			stdio: ["ignore", "pipe", "ignore"],
			timeout: 10_000,
		}).trim();
		return out || undefined;
	} catch {
		// 本地没有 stable / main 分支时无法判定，交给远端分支保护与 CI 守卫。
		return undefined;
	}
}

export default function guard(pi: ExtensionAPI): void {
	pi.on("tool_call", async (event, ctx) => {
		if (event.toolName !== "bash") return;
		const command = event.input.command;
		if (typeof command !== "string") return;
		const cwd = typeof event.input.cwd === "string" && event.input.cwd ? resolve(ctx.cwd, event.input.cwd) : ctx.cwd;
		const caller: Caller = { isSub: ctx.agent?.kind === "sub", name: (ctx.agent?.name ?? "").toLowerCase() };
		const verdict = checkBash(command, caller, dir => stableDivergence(dir ? resolve(cwd, dir) : cwd));
		if (!verdict) return;
		if ("block" in verdict) return verdict;
		if (ctx.hasUI && (await ctx.ui.confirm("推送发布 tag？", `${verdict.confirm}\n\n${command}`))) return;
		return { block: true, reason: `${verdict.confirm} 未获用户确认，已拦截。` };
	});
}
