// FluxDown 插件页面脚本桥（`contributes.pageScripts`）的全局类型。
//
// 在页面脚本文件里引用：`/// <reference types="fluxdown-plugin-sdk/types/page" />`，
// 或在 tsconfig 的 `types` 中加入 `fluxdown-plugin-sdk/types/page`。
//
// 只有 `world: "isolated"`（默认）的脚本有 `fluxdown`；`world: "main"` 的脚本运行在页面自身的
// JS world，没有这个桥（网页可以伪造任何消息，桥不能暴露给它）。需要读页面 JS 状态时，
// 成对提供一个 main 脚本和一个 isolated 脚本，经 DOM CustomEvent 通信，isolated 一侧把收到的数据当不可信输入。
//
// 每次桥调用失败都会 reject 一个带可读消息的 `Error`。后台会校验 payload，单条消息 JSON ≤ 256 KiB，并按插件限流。

declare const fluxdown: {
  /** 当前脚本的身份信息。 */
  readonly info: {
    /** 插件 identity，例如 `fluxdown@feature-demo`。 */
    readonly identity: string;
    /** manifest 中该页面脚本的 `id`。 */
    readonly scriptId: string;
    /** 插件版本。 */
    readonly version: string;
  };

  /**
   * 交给 FluxDown 下载。`url` 仅限 http/https；`headers` 仅限
   * referer / origin / user-agent / accept / accept-language / x-requested-with；
   * `referrer` 缺省为当前页面地址。
   */
  download(request: {
    url: string;
    filename?: string;
    referrer?: string;
    headers?: Record<string, string>;
  }): Promise<{ ok: boolean; message?: string }>;

  /**
   * 把资源加入本标签页的嗅探媒体列表（与嗅探结果同一列表，并标注来源插件），返回被接受的条数。
   * `url` 仅限 http/https，单次最多 100 条。
   */
  addMedia(items: Array<{ url: string; title?: string; mime?: string }>): Promise<number>;

  /**
   * 调用本插件后台通过 `flux.rpc.handle(name, fn, { page: true })` 注册的处理器。
   * 处理器收到 `(args, { caller: "page", pageUrl })`；未以 `page: true` 注册的处理器会被拒绝。
   */
  invoke<T = unknown>(name: string, args?: unknown): Promise<T>;

  /** 按插件隔离的持久存储（扩展 `storage.local`；单插件总量 ≤ 1 MiB，值须可 JSON 序列化）。 */
  storage: {
    get<T = unknown>(key: string): Promise<T | undefined>;
    set(key: string, value: unknown): Promise<void>;
    remove(key: string): Promise<void>;
  };
};
