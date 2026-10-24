---
title: Page Scripts
description: Run plugin scripts on web pages through the FluxDown browser extension — declare them, use the fluxdown bridge, pair main and isolated worlds, and know the limits.
section: plugins
order: 4.8
---

A **page script** is a JavaScript file your plugin ships and the FluxDown browser extension runs on the websites you list — for example to add a "download" button to a video site, or to pass a link found on the page to your plugin's `main` script. The extension fetches the code from the FluxDown desktop app and registers it with the browser's own `userScripts` API.

The manifest fields are in the [manifest reference](/docs/en/plugins/manifest/#pagescripts); the permission is `pageScripts` (**Full** risk, see [Permissions](/docs/en/plugins/permissions/#page-scripts-in-the-browser)).

## Declare a page script

```json
{
  "identity": "acme@page-demo",
  "name": "Page demo",
  "version": "1.0.0",
  "main": { "entry": "main.js" },
  "permissions": ["pageScripts"],
  "contributes": {
    "pageScripts": [{
      "id": "hello",
      "title": "Hello button",
      "matches": ["https://example.com/*"],
      "js": ["page/hello.js"]
    }]
  }
}
```

`main.js` exposes a handler to the page with `page: true`:

```js
flux.rpc.handle("lookup", async (args, ctx) => {
  // ctx = { caller: "page", pageUrl } — both come from a web page: untrusted.
  const url = typeof args?.url === "string" ? args.url : "";
  if (!/^https:\/\//.test(url)) return { ok: false, error: "https only" };
  return { ok: true, title: "found", from: ctx.pageUrl };
}, { page: true });
```

`page/hello.js` runs on `https://example.com/*` and calls it:

```js
(async () => {
  const reply = await fluxdown.invoke("lookup", { url: location.href });
  if (reply.ok) {
    await fluxdown.download({ url: location.href });
  }
})();
```

Tasks the handler creates with `flux.tasks.create` or resumes with `flux.tasks.resume` during the call get the same desktop prompts as a download captured straight from the browser: when exactly one task starts and the FluxDown main window is not in front, the progress and completion windows appear according to the "Notifications → Download windows" settings. Starting several tasks at once shows no window. `await` those calls before the handler returns; calls that finish after it returns are not counted.

The full runnable version is in the [feature demo plugin](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk/examples/feature-demo) (`page/hello.js`).

## Choosing a world

| `world` | Where it runs | `fluxdown` bridge |
|---|---|---|
| `isolated` (default) | The extension's user-script world: its own JavaScript globals, the page's DOM | **Yes** |
| `main` | The page's own JavaScript world: it sees the page's variables and functions | **No** |

A `main` script has no bridge on purpose: anything in that world can be observed and forged by the page itself, so a bridge there could be abused by the site. Use `main` only to read or call page JavaScript.

### Pairing a main and an isolated script

When you need page JavaScript state (say a player object) *and* the bridge, declare two scripts, one per world, and talk through DOM events:

```json
"pageScripts": [
  { "id": "probe", "title": "Probe", "matches": ["https://example.com/*"], "js": ["page/probe-main.js"], "world": "main" },
  { "id": "bridge", "title": "Bridge", "matches": ["https://example.com/*"], "js": ["page/probe-bridge.js"] }
]
```

```js
// page/probe-main.js — world: main
document.dispatchEvent(new CustomEvent("acme-demo:state", { detail: JSON.stringify({ id: window.player?.id ?? null }) }));
```

```js
// page/probe-bridge.js — world: isolated
document.addEventListener("acme-demo:state", async (event) => {
  let state;
  try { state = JSON.parse(String(event.detail)); } catch { return; }
  if (typeof state?.id !== "string") return;   // the page can send anything: validate
  await fluxdown.invoke("lookup", { url: `https://example.com/v/${encodeURIComponent(state.id)}` });
});
```

The isolated side must treat every event as **untrusted input**: the page (or other scripts on it) can dispatch the same event with anything inside.

## The `fluxdown` bridge

Available only in `isolated` scripts. Every call returns a promise that rejects with an `Error` carrying a readable message when it fails.

| API | What it does |
|---|---|
| `fluxdown.info` | `{ identity, scriptId, version }` of the running script. |
| `fluxdown.download({ url, filename?, referrer?, headers? })` | Hands a download to FluxDown. `url` must be `http`/`https`. `headers` may only contain `referer`, `origin`, `user-agent`, `accept`, `accept-language`, `x-requested-with`. `referrer` defaults to the current page. Resolves `{ ok, message? }`. |
| `fluxdown.addMedia([{ url, title?, mime? }])` | Adds resources to this tab's sniffed media list (the same list as sniff results, labelled with your plugin). `http`/`https` only, at most 100 per call. Resolves with the number accepted. |
| `fluxdown.invoke(name, args?)` | Calls a handler your `main` registered with `flux.rpc.handle(name, fn, { page: true })`. |
| `fluxdown.storage.get(key)` / `set(key, value)` / `remove(key)` | Per-plugin persistent storage in the extension. Values must be JSON-serializable. |

TypeScript declarations are in `fluxdown-plugin-sdk/types/page` (`/// <reference types="fluxdown-plugin-sdk/types/page" />`).

## Limits

| Limit | Value |
|---|---|
| Page scripts per plugin | 16 |
| `matches` / `excludeMatches` per script | 32 each |
| `js` files per script | 8 |
| One `js` file | 256 KiB |
| One script, files joined | 384 KiB |
| One bridge message (JSON) | 256 KiB |
| `fluxdown.addMedia` per call | 100 items |
| `fluxdown.storage` per plugin | 1 MiB |

Bridge calls are validated by the extension and rate-limited per plugin. The script runs in a world whose content security policy forbids evaluating strings as code: `eval` and `new Function` do not work in `isolated` scripts, so ship plain code instead of building it at runtime.

## What the user sees and when updates apply

1. The user approves the `pageScripts` permission in FluxDown (the dialog lists each script's title and sites).
2. In the browser extension's options page, the user turns page scripts on for your plugin — the extension asks once, per plugin, and explains the risk. The browser also needs its user-scripts switch on (see [Plugin page scripts](/docs/en/browser-extension/usage/#plugin-page-scripts)).
3. From then on the extension keeps itself in line with FluxDown: a new version of your plugin, a new script, a changed `matches` list or new code all take effect automatically, **without** asking again. Disabling or uninstalling the plugin removes the scripts.

The extension always takes the code from the desktop app and does not keep a source copy of its own. If the desktop app is not reachable, scripts already registered keep running.

## Safety notes

- A page script can do everything the page's user can do on those sites. Match the **smallest** set of sites you need and add `excludeMatches` for areas you do not touch.
- Everything coming from the page is untrusted: DOM contents, events, `ctx.pageUrl`, `args`. Validate in the handler in `main` as well as in the script.
- Expose a handler with `page: true` only when it is safe for any script on a matching page to call it. A page script that can call `flux.tasks`-powered handlers can create downloads on the user's behalf.
- Never put secrets in a page script or in `fluxdown.storage`; both are readable by the page's environment in the worst case.
- Review the plugin like code you would run in the browser console of those sites, because that is what it is.

## Errors you will meet

| Message | Cause |
|---|---|
| `声明 pageScripts 时 permissions 必须包含 pageScripts` | The manifest declares page scripts without the `pageScripts` permission. |
| `包内缺少文件 '<path>'` | A `js` file is not in the package. |
| `rpc not exposed to page: <name>` | The handler was registered without `{ page: true }`. |
| `rpc not registered: <name>` | `main` has no handler by that name (or the plugin is disabled). |

More validation messages are listed in [Debugging](/docs/en/plugins/debugging/).
