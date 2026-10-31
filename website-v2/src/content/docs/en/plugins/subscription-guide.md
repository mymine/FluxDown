---
title: Subscription Guide
description: Turn any website or API into a subscription feed — the subscriptions[] manifest entry, the subscribe(ctx) contract, validation rules and pairing with a resolver.
section: plugins
order: 6
---

FluxDown's RSS feature watches a *source* on a schedule, remembers which items it has seen, applies the user's filters and creates download tasks for new items. Out of the box a source is an RSS, Atom or JSON Feed URL. A **subscription provider** is a plugin that replaces the "fetch and parse" step, so a source can be anything you can read with `flux.fetch`: a JSON API, an HTML listing, a release page.

Everything else — scheduling, de-duplication, filters, the first-round rule, retries with back-off, task creation — stays FluxDown's job. Your plugin only turns *one source URL* into *a list of items*.

This guide explains how users point a source at your provider, the manifest entry, the complete `subscribe(ctx)` contract with every validation rule, two worked examples (one of them paired with a resolver), and how to test. The manifest fields are also listed in the [manifest reference](/docs/en/plugins/manifest/).

## How the pieces fit

| Piece | Who | Job |
|---|---|---|
| Source | the user | A record with a URL, a `providerId` and an optional `providerConfig`, plus the usual RSS options (interval, filters, save folder, queue, auto-download, cookies, user agent). |
| Provider | your plugin | `subscribe(ctx)`: fetch the source, return `{ title, link, items }`. |
| Engine | FluxDown | Calls the provider on schedule, validates the result, stores new items, evaluates filters, creates tasks. |
| Resolver (optional) | your plugin or another | Turns an item's page `link` into a real download when the item has a `resolverItem`. |

The built-in provider has the fixed id `rss`. A source whose `providerId` is another value is handed to the plugin that declares that id.

### How users create a source for your provider

Plugin-backed sources are ordinary subscriptions: they appear in the RSS page with their items, **Refresh**, last-error text and per-source options, and all of the user's filters (include/exclude keywords, size range, smart episode de-duplication), queue and save-folder settings apply.

A source needs the pair `providerId` + `providerConfig`. Users can create one in three places, and all of them write the same record:

- **Desktop app and web UI.** On the RSS page choose **New subscription**, then pick your plugin's name under **Subscription provider**. The selector is listed first with the built-in **RSS / Atom / JSON Feed** and shows one entry per plugin provider that is currently usable: the plugin must be installed, enabled, loaded, and have its permissions and network hosts authorised. If no plugin provider is usable the selector is hidden and the form is the plain RSS form. For a plugin provider the form asks for a **Source URL** (handed to you as `ctx.url`) and an optional **Provider configuration (JSON)** (handed to you as `ctx.providerConfig`). Leave the configuration empty or enter valid JSON — anything else is refused before saving. As for RSS, a new source has to pass **Verify** first: FluxDown calls your `subscribe` once and shows the feed title and item count, or your error text. Opening an existing source shows its provider; if that plugin is missing, disabled or not yet authorised, the selector keeps the stored `providerId` and explains why, instead of silently switching to RSS, and the source reports a fetch error until the plugin is available again.
- **Command line.** `fluxdown rss add <URL> --provider <providerId> --provider-config '<JSON>'`. `--provider-config` is optional, must be valid JSON and requires `--provider`. Without `--provider` the source uses the built-in RSS provider. The command creates the source and starts a first fetch right away; check it with `fluxdown rss items <sourceId>` and the `STATE` column of `fluxdown rss ls`.
- **Management API.** `POST /api/v1/rss` (or the `daemon.rss.createSource` RPC) accepts the two fields directly, and `POST /api/v1/rss/validate` accepts `providerId` and `providerConfig` for the same dry run as **Verify**:

```bash
curl -X POST http://127.0.0.1:17800/api/v1/rss \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
        "providerId": "acme-releases",
        "providerConfig": "{\"assetPattern\":\"\\\\.zip$\"}",
        "url": "https://releases.example.com/api/releases?project=foo",
        "intervalMinutes": 60,
        "autoDownload": true
      }'
# -> {"sourceId":"…"}
```

`providerConfig` is an opaque **string** whose meaning you define; the user interface and the command line only accept valid JSON (or nothing), so JSON is the convention to follow. The API itself stores any string. `url` is passed to you unchanged. The management API must be enabled and the token set; see the [API overview](/docs/en/api/overview/). A provider that is not installed or not enabled makes the source fail with a visible error until it is available. `GET /api/v1/plugins` lists each plugin's `subscriptionProviderIds`.

Behaviour of the schedule that your provider should know about:

- **First round never downloads.** The first successful fetch only records the current items as already seen (marked read). Only items that appear in later fetches become downloads. To see items in the list without downloading anything, create the source with `"autoDownload": false` (collect mode).
- **Interval.** Default 30 minutes, minimum 1 minute. After a failure the interval doubles for each consecutive failure up to 6 hours (never shorter than the configured interval); it resets after a success.
- **De-duplication is by `guid`** within a source. Per fetch at most `maxPerFetch` (default 20, 1–100) new items become tasks, oldest first by `pubDate`. FluxDown keeps the 500 newest entries per source.
- `flux.fetch` and `flux.ytdlp` follow the source's own proxy setting when it has one; otherwise site rules matching the source URL and the application's global proxy settings (including Auto switching to a proxy when the direct route fails) apply.

## Declare the provider

```json
"subscriptions": [
  { "providerId": "acme-releases", "entry": "subscribe.js", "timeoutMs": 20000 }
]
```

| Field | Rules |
|---|---|
| `providerId` | Required. `^[a-z0-9_-]{1,64}$`. Stable: it is stored in every source that uses it. **`rss` is reserved** for the built-in provider and rejected. If two enabled plugins declare the same id, the one with the alphabetically smaller `identity` wins, so choose an id that is unlikely to collide. |
| `entry` | Required. Script file, safe relative path. It must define the global function **`subscribe`** — the name is fixed. It is compile-checked at install time. |
| `timeoutMs` | Optional. Wall-clock limit of one call. Default **10 s**; set it up to the **30 s** ceiling. Must not be `0`. |

At most **one** provider per plugin. A plugin may combine `subscriptions` with `resolvers`, `hooks`, `auth`, settings and so on. `required` settings are enforced: if one has no value the call fails with `缺少必填设置项: 插件 <identity> 需先配置「<title>」` (“missing required setting: the plugin needs <title> configured first”).

The calls share the resolver's runtime: 64 MB memory, and if no execution slot frees up within 3 s the call fails with `插件运行时过载（并发已满）` (“the plugin runtime is overloaded”). Three consecutive timeouts or out-of-memory failures trip the circuit breaker and disable the plugin, like for resolvers.

## The `subscribe(ctx)` contract

```js
async function subscribe(ctx) {
  // ... fetch and normalize ...
  return { title: "…", link: "…", items: [ /* … */ ] };
}
```

### `ctx`

| Field | Meaning |
|---|---|
| `providerId` | Your provider id (useful if one script serves several). |
| `sourceId` | The source record id; a good key for `flux.storage`. |
| `url` | The source URL exactly as stored. |
| `providerConfig` | The opaque config string. `"{}"` when the user gave none, so `JSON.parse(ctx.providerConfig)` is always safe on valid JSON. |
| `cookies` | The source's cookie string (a `Cookie` header value), or `""`. |
| `userAgent` | The user agent for the source (source setting, else the global one). |

Available facades: `flux.fetch`, `flux.storage`, `flux.fs`, `flux.logger`, `flux.settings`, `flux.info`, plus `flux.auth` (with `auth` permission, credentials are attached to `flux.fetch` automatically — see the [Auth guide](/docs/en/plugins/auth-guide/)) and `flux.ytdlp` (with `ytdlp`). There is no `flux.ffmpeg`, `flux.task` or resident-script facade. Cookies and the user agent are **not** added to your requests for you; pass them yourself if the site needs them.

### Return value

Return an **object**. Returning `null`/`undefined` is an error (`订阅返回值非法: …`).

| Field | Type | Meaning |
|---|---|---|
| `title` | string | Source title. If the user left the source name empty, it is filled from this once, after the first success. |
| `link` | string | The source's home page. Informational. |
| `items` | array | At most **1000** items. Missing = empty feed. |

Each item:

| Field | Type | Rules |
|---|---|---|
| `guid` | string | **Required**, non-empty, at most 2048 **bytes**. The de-duplication key: the same `guid` is never turned into a second task, so it must stay stable between fetches and be unique within the result. |
| `title` | string | At most 1024 bytes. What the include/exclude filters and smart-episode detection read; give it a meaningful, non-empty text. |
| `link` | string | Page or direct URL. |
| `enclosureUrl` | string | The direct download URL. Preferred over `link` when both are present. |
| `resolverItem` | string | At most 2048 bytes. Opaque token for a resolver; **requires `link`**. See [Pairing with a resolver](#pairing-with-a-resolver). |
| `enclosureLength` | integer | Size in bytes, `0` if unknown, never negative. Feeds the user's size filter. |
| `pubDate` | integer | Publication time in **Unix seconds**, `0` if unknown. Sets the order in which new items are downloaded (oldest first). |

Rules beyond the table:

1. At least one of `link` and `enclosureUrl` is required.
2. Every URL must start with `http:`, `https:`, `ftp:`, `magnet:` or `ed2k:` and be at most 8 KB. Anything else (`javascript:`, `file:`, relative URLs) invalidates the item.
3. **Types are strict.** `guid`, `title`, `link`, `enclosureUrl`, `resolverItem` must be strings (not numbers, not `null`); `enclosureLength` and `pubDate` must be integers (use `Math.round`/`Math.floor`; `"2025-01-01"` or `12.5` are errors). A wrong type fails the **whole** result with `订阅返回值非法: …`. Leave a field out instead of setting it to `null`.
4. **Bad items are skipped, not fatal.** One invalid item is skipped and logged (`[plugin] subscription output: skipped N of M items, first reason: …`); the rest are used. A single dirty row upstream should not push a source into back-off. But if the list is non-empty and **every** item is invalid, the call fails (`订阅条目全部非法（N 条），首条原因: …`). More than 1000 items fails too (`订阅条目数量超过 1000`).
5. Download address of an item: with a `resolverItem`, the `link` (through a resolver); otherwise `enclosureUrl`, falling back to `link`. `.torrent` URLs and magnet links are handled by FluxDown's BitTorrent engine like any other.

The reasons logged for skipped items are:

| Reason | Meaning |
|---|---|
| `guid 必须非空且不超过 2048 字节` | “guid must be non-empty and at most 2048 bytes” |
| `条目 <guid> 的 title 超过 1024 字节` | “title exceeds 1024 bytes” |
| `条目 <guid> 的 resolverItem 超过 2048 字节` | “resolverItem exceeds 2048 bytes” |
| `条目 <guid> 缺少 link / enclosureUrl` | “has neither link nor enclosureUrl” |
| `条目 <guid> 含 resolverItem 时必须提供 link` | “resolverItem requires link” |
| `条目 <guid> 的 link 非法: 插件输出非法: url scheme 不允许: <scheme>` | “link is invalid: scheme not allowed” (or `url 超过 8KB`, “url exceeds 8 KB”); the same for `enclosureUrl` |
| `条目 <guid> 的 enclosureLength 不可为负数` | “enclosureLength must not be negative” |

## Example 1: a JSON API as a feed

A fictional release API at `https://releases.example.com/api/releases?project=foo` answers

```json
{
  "project": "foo",
  "homepage": "https://releases.example.com/foo",
  "releases": [
    {
      "id": 412,
      "name": "v2.4.0",
      "published": "2025-03-02T10:15:00Z",
      "page": "https://releases.example.com/foo/v2.4.0",
      "assets": [
        { "name": "foo-2.4.0-win.zip", "url": "https://dl.example.com/foo/2.4.0/foo-2.4.0-win.zip", "size": 48211967 },
        { "name": "foo-2.4.0-mac.dmg", "url": "https://dl.example.com/foo/2.4.0/foo-2.4.0-mac.dmg", "size": 51200333 }
      ]
    }
  ]
}
```

Every asset of every release becomes an item. `providerConfig` carries an optional file-name filter: `{"assetPattern": "\\.zip$"}`.

`manifest.json`

```json
{
  "identity": "acme@release-feed",
  "name": "Release Feed",
  "version": "1.0.0",
  "description": "Subscribe to the assets of a project on releases.example.com.",
  "subscriptions": [
    { "providerId": "acme-releases", "entry": "subscribe.js", "timeoutMs": 20000 }
  ]
}
```

`subscribe.js`

```js
function parseConfig(raw) {
  try {
    return JSON.parse(raw || "{}");
  } catch (e) {
    throw new Error("providerConfig is not valid JSON: " + e.message);
  }
}

function normalize(data, config) {
  const pattern = config.assetPattern ? new RegExp(config.assetPattern, "i") : null;
  const items = [];
  for (const release of data.releases || []) {
    for (const asset of release.assets || []) {
      if (pattern && !pattern.test(asset.name)) continue;
      items.push({
        // Stable and unique per asset: never reuse a guid for different files.
        guid: release.id + "/" + asset.name,
        title: data.project + " " + release.name + " - " + asset.name,
        link: release.page,
        enclosureUrl: asset.url,
        enclosureLength: Math.max(0, Math.round(asset.size || 0)),
        // Unix seconds; 0 when the date cannot be parsed.
        pubDate: Math.floor(Date.parse(release.published) / 1000) || 0,
      });
    }
  }
  return { title: data.project + " releases", link: data.homepage || "", items };
}

async function subscribe(ctx) {
  const config = parseConfig(ctx.providerConfig);
  const headers = { Accept: "application/json", "User-Agent": ctx.userAgent };
  if (ctx.cookies) headers.Cookie = ctx.cookies;

  const res = await flux.fetch({ url: ctx.url, headers });
  if (res.status !== 200) throw new Error("releases API answered HTTP " + res.status);
  if (res.truncated) throw new Error("the response exceeded the 8 MB limit");

  return normalize(JSON.parse(res.body), config);
}
```

Notes:

- Throwing is the right way to report a failed fetch: the message becomes the source's *last error* (prefixed `插件运行时错误: `) and the back-off applies.
- `flux.fetch` reads at most 8 MB per response, 10 s per request, and only public `http`/`https` hosts; check `res.truncated`.
- A `normalize` function that takes plain data is easy to unit-test outside FluxDown (see [Testing](#testing)).
- There is no DOM in the plugin runtime. For HTML pages keep the scraping to a few anchored `RegExp`s (the episode example below does this), and prefer a JSON endpoint whenever the site offers one.

## Pairing with a resolver

Sometimes an item's final link is only known at download time — a signed URL that would expire in the feed, or a page that offers several qualities. Return the **page** as `link` and set `resolverItem`. When FluxDown creates the task it uses the `link`, finds the resolver plugin whose `resolvers[0].match.urls` matches it, and calls that resolver with `ctx.resolverItem` set to your token (a second-stage resolve). The resolver turns it into the real file.

Rules for the pairing:

- `resolverItem` is opaque to FluxDown. You define its meaning (a quality, an id, `<id>@<variant>`…) and your resolver reads it.
- It requires `link`; an item with a `resolverItem` and no `link` is skipped.
- If no installed, enabled plugin's resolver matches the `link`, the task is created already **failed** with `解析插件不可用` (“the resolver plugin is unavailable”). It never downloads the page as if it were a file.
- Second-stage resolves are unattended: a resolver that returns `variants` does not show a picker; the entry at `defaultVariantIndex` is used. A second-stage resolve must **not** return a `manifest` (`二段解析（resolverItem 非空）不可返回 manifest：防递归裂变`, “second stage must not return a manifest: no recursive fan-out”).
- The resolver can be your own (same plugin, as below) or another plugin's.

### Worked example: an episode listing

`tv.example.com/shows/42` lists episodes as links like `/watch/7001`. The user chooses a quality in `providerConfig` (`{"quality":"1080p"}`). The provider yields one item per episode; the resolver, in the same plugin, picks the matching stream for that episode when the task starts.

`manifest.json`

```json
{
  "identity": "acme@tv-episodes",
  "name": "TV Episodes",
  "version": "1.0.0",
  "description": "Follow a show on tv.example.com and download new episodes in your preferred quality.",
  "resolvers": [
    {
      "match": { "urls": ["*://tv.example.com/watch/*"] },
      "entry": "resolve.js"
    }
  ],
  "subscriptions": [
    { "providerId": "acme-tv", "entry": "subscribe.js" }
  ]
}
```

`subscribe.js`

```js
async function subscribe(ctx) {
  const config = JSON.parse(ctx.providerConfig);
  const quality = config.quality || "720p";

  const res = await flux.fetch({ url: ctx.url, headers: { "User-Agent": ctx.userAgent } });
  if (res.status !== 200) throw new Error("listing answered HTTP " + res.status);

  const origin = ctx.url.match(/^https?:\/\/[^/]+/)[0];
  const title = (res.body.match(/<h1[^>]*>([^<]+)<\/h1>/) || [])[1] || "TV show";

  const items = [];
  const re = /<a class="ep" href="(\/watch\/(\d+))"[^>]*>([^<]+)<\/a>/g;
  let m;
  while ((m = re.exec(res.body)) !== null) {
    items.push({
      guid: "episode-" + m[2],          // one task per episode, whatever the quality
      title: title.trim() + " - " + m[3].trim(),
      link: origin + m[1],              // the page, resolved at download time
      resolverItem: quality,            // handed to resolve() as ctx.resolverItem
    });
  }
  return { title: title.trim(), link: ctx.url, items };
}
```

`resolve.js`

```js
async function resolve(ctx) {
  const id = (ctx.url.match(/\/watch\/(\d+)/) || [])[1];
  if (!id) return null;

  // Subscription items carry the wanted quality; a link added by hand has none.
  const wanted = ctx.resolverItem || "720p";

  const res = await flux.fetch({
    url: "https://tv.example.com/api/episodes/" + id + "/sources",
    headers: { Referer: ctx.url },
  });
  if (res.status !== 200) throw new Error("sources API answered HTTP " + res.status);

  const sources = JSON.parse(res.body).sources || [];
  const pick = sources.find((s) => s.quality === wanted) || sources[0];
  if (!pick) throw new Error("no playable source for episode " + id);

  return { url: pick.url, fileName: "episode-" + id + "-" + pick.quality + ".mp4", ephemeral: true };
}
```

Because the page URL stays in the feed and the signed stream URL is produced only when the task starts (and again on every resume), links never expire inside the item list.

## Testing

**Unit-test the mapping outside FluxDown.** Keep fetching and normalizing separate, as in Example 1, and run the script in Node or Bun with a fake `flux`:

```js
// test.mjs — node test.mjs
import fs from "node:fs";
import vm from "node:vm";

const sample = { project: "foo", releases: [{ id: 1, name: "v1", published: "2025-03-02T10:15:00Z",
  page: "https://releases.example.com/foo/v1",
  assets: [{ name: "foo.zip", url: "https://dl.example.com/foo.zip", size: 10 }] }] };

const flux = {
  fetch: async () => ({ status: 200, headers: {}, truncated: false, body: JSON.stringify(sample) }),
};
const sandbox = vm.createContext({ flux, JSON, Date, Math, RegExp, Error });
vm.runInContext(fs.readFileSync("subscribe.js", "utf8"), sandbox);

const out = await sandbox.subscribe({
  url: "https://releases.example.com/api/releases?project=foo",
  providerConfig: '{"assetPattern":"\\\\.zip$"}',
  cookies: "",
  userAgent: "test",
});
console.log(JSON.stringify(out, null, 2));
```

Check by eye (or in code) that every `guid` is a unique string, every URL starts with an allowed scheme, and numbers are integers.

**End to end inside FluxDown:**

1. Install the folder in dev mode. `subscribe.js` is re-read on every call; after editing `manifest.json` press **Reload** (`重新加载`) on the plugin row.
2. Create a source with the `curl` command above, with `"autoDownload": false` so nothing downloads while you iterate.
3. Trigger a fetch at will with `POST /api/v1/rss/{sourceId}/refresh`, then read the result with `GET /api/v1/rss/{sourceId}/items` and the source's `lastError` with `GET /api/v1/rss`. The plugin log shows your `flux.logger` lines ([Debugging](/docs/en/plugins/debugging/)).
4. The first fetch only seeds. To watch new items arrive, delete and re-create the source after changing the upstream data, or switch to `autoDownload: true` and wait for the next change.
5. Paired resolver: put one item's `link` into a normal download and check that `ctx.resolverItem` is empty there; then let the subscription create the task and confirm it arrives set.

## Errors

The text in the source's last error is the engine message (Chinese), exactly as shown:

| Message | Cause |
|---|---|
| `订阅 provider <id> 未安装或已禁用` (“provider not installed or disabled”) | No enabled plugin declares that `providerId` — including after the circuit breaker disabled it. |
| `订阅 provider <id> 的脚本读取失败` (“the provider script could not be read”) | Missing or unreadable `entry` file. |
| `入口 subscribe 未定义` (“entry subscribe is not defined”) | The script has no global `subscribe` (a `const` arrow function does not count — use `async function subscribe`). |
| `订阅返回值非法: …` (“invalid subscription return value”) | `null`/`undefined`, not an object, or a field with the wrong type. The tail is the parser's description. |
| `订阅条目数量超过 1000` | More than 1000 items. |
| `订阅条目全部非法（N 条），首条原因: …` (“all N items are invalid, first reason: …”) | Every item failed validation; see the reasons table above. |
| `缺少必填设置项: 插件 … 需先配置「…」` | A `required` setting has no value. |
| `脚本执行超时` (“script execution timed out”) | Exceeded `timeoutMs` (default 10 s, ceiling 30 s). |
| `插件运行时错误: <your message>` (“plugin runtime error”) | An exception thrown by your script or a rejected `flux.*` call. |

See also the [Auth guide](/docs/en/plugins/auth-guide/) for sites that need a login, the [Hooks guide](/docs/en/plugins/hooks-guide/) to react to the downloads a subscription creates, and [Your first plugin](/docs/en/plugins/your-first-plugin/) for resolver basics.
