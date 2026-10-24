// 测试用的 `flux` 宿主替身：内存 KV / 工作区、按路由应答的 fetch、收集日志。
export type FetchRequest = { method: string; url: string; headers: Record<string, string>; body?: string };
type Response = { status: number; headers?: Record<string, string>; body: string };
export type Route = (req: FetchRequest) => Response | undefined;

type SolverRequest = { type: string; challenges: string[] };
export type SolverInput = { type: string; player?: string; output_preprocessed?: boolean; requests: SolverRequest[] };
export type SolverRecord = { calls: { type: string; requests: SolverRequest[] }[] };

// 插件源码以全局 `flux` / `ytSolver` 访问宿主与求解器（引擎注入 / bundle 顶部定义），入口挂在
// `globalThis.resolve`；测试经这一个视图装替身、调入口。
export const host = globalThis as unknown as {
  flux: unknown;
  ytSolver: (input: SolverInput) => unknown;
  resolve: (ctx: Record<string, unknown>) => Promise<unknown>;
};

export interface Shim {
  storage: Map<string, string>;
  files: Map<string, string>;
  requests: FetchRequest[];
  logs: string[];
}

export function installFlux(routes: Route[] = [], settings: Record<string, unknown> = {}): Shim {
  const shim: Shim = { storage: new Map(), files: new Map(), requests: [], logs: [] };
  host.flux = {
    async fetch(opts: { method?: string; url: string; headers?: Record<string, string>; body?: string }) {
      const req: FetchRequest = { method: opts.method || "GET", url: opts.url, headers: opts.headers || {}, body: opts.body };
      shim.requests.push(req);
      for (const route of routes) {
        const res = route(req);
        if (res) return { headers: {}, truncated: false, ...res };
      }
      return { status: 404, headers: {}, body: "", truncated: false };
    },
    storage: {
      async get(key: string) {
        return shim.storage.get(key) ?? null;
      },
      async set(key: string, value: string) {
        shim.storage.set(key, String(value));
      },
    },
    fs: {
      async writeFile(name: string, content: string) {
        shim.files.set(name, content);
      },
      async readFile(name: string) {
        return shim.files.get(name) ?? null;
      },
      async remove(name: string) {
        shim.files.delete(name);
      },
      async list() {
        return [...shim.files.keys()];
      },
    },
    settings,
    logger: {
      info: (...a: unknown[]) => shim.logs.push(a.join(" ")),
      warn: (...a: unknown[]) => shim.logs.push(a.join(" ")),
      error: (...a: unknown[]) => shim.logs.push(a.join(" ")),
    },
  };
  return shim;
}

/** 假求解器：n → `N(<挑战>)`，签名 → 反转字符串；记录每次调用的输入类型与挑战。 */
export function installSolver(): SolverRecord {
  const record: SolverRecord = { calls: [] };
  host.ytSolver = (input) => {
    record.calls.push({ type: input.type, requests: input.requests });
    const [n, sig] = input.requests;
    return {
      type: "result",
      ...(input.type === "player" && input.output_preprocessed ? { preprocessed_player: `pre:${input.player}` } : {}),
      responses: [
        { type: "result", data: Object.fromEntries((n?.challenges ?? []).map((c) => [c, `N(${c})`])) },
        { type: "result", data: Object.fromEntries((sig?.challenges ?? []).map((c) => [c, [...c].reverse().join("")])) },
      ],
    };
  };
  return record;
}
