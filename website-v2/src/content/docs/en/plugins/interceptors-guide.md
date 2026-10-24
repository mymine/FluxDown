---
title: Interceptors Guide
description: Read and change a running download — rewrite requests, set connection counts, pick streaming variants, filter segments, post-process the file and decide retries.
section: plugins
order: 4.5
---

Interceptors give a plugin a hook *into* the engine's task pipeline. Where [hooks](/docs/en/plugins/hooks-guide/) only react after the fact, an interceptor sees the information the engine holds for a task and can change it before the engine uses it: the request (URL, method, body, headers, cookies, referrer, User-Agent, per-site headers), the connection count, the streaming variant, individual segments, the finished file, and whether a failed task should be retried.

Manifest fields are in the [manifest reference](/docs/en/plugins/manifest/#interceptors); the required permission is `intercept` (elevated risk, see [Permissions](/docs/en/plugins/permissions/)).

## Declare and write the plugin

```json
{
  "identity": "acme@demo-intercept",
  "name": "Demo Interceptor",
  "version": "1.0.0",
  "permissions": ["intercept"],
  "interceptors": {
    "entry": "intercept.js",
    "stages": ["prepare", "segments"],
    "match": { "urls": ["https://example.com/*"] }
  }
}
```

The entry script defines **one** global function `intercept(ctx)`. FluxDown calls it once per stage the plugin subscribed to; branch on `ctx.stage`. Return `null` / `undefined` to change nothing, otherwise the stage's result object. All field names are camelCase. Every call runs in a fresh script context.

```js
globalThis.intercept = function (ctx) {
  if (ctx.stage === "prepare") {
    return { userAgent: "Demo/1", setHeaders: { "X-Demo": "on" }, segments: 4 };
  }
  if (ctx.stage === "segments") {
    return { drop: ctx.segments.filter((s) => s.tags.some((t) => t.startsWith("#EXT-X-CUE-OUT"))).map((s) => s.id) };
  }
  return null;
};
```

With the TypeScript SDK use `defineInterceptors`, which installs the entry for you and types each stage's context and result:

```ts
import { defineInterceptors } from "fluxdown-plugin-sdk/runtime";

defineInterceptors({
  prepare: (ctx) => ({ setHeaders: { "X-Demo": "on" } }),
  segments: (ctx) => ({ drop: ctx.segments.filter((s) => s.duration === 0).map((s) => s.id) }),
});
```

## Which tasks, which order

A task is intercepted by an enabled plugin that holds the `intercept` permission and whose `match.urls` matches the task's original URL or its resolver-rewritten URL. When several plugins match they run in identity order, each seeing the previous plugin's changes. `ctx.task` always carries `id`, `url` (this run's URL), `sourceUrl` (the persisted URL), `fileName`, `saveDir`, `protocol` (`http`, `ftp`, `hls`, `dash`), `trackPair` and `isResume`. BitTorrent and eD2K tasks are not intercepted.

If a call throws, times out or returns something invalid, `onFailure` decides: `continue` records a note in the task's activity log (kind **Plugin**) and skips that plugin; `fail` fails the task.

## Stages

| Stage | When | Return |
|---|---|---|
| `prepare` | Before the request is built — on every run: first start, retry, resume, restart. Also before the metadata probe. | `RequestPatch` |
| `probed` | After the HTTP / FTP probe, before segments are planned. Context adds `probe` (`totalBytes`, `supportsRange`, `fileName`, `contentType`, `etag`, `lastModified`). | `{ segments?, maxConnections?, fail? }` |
| `refresh` | A request was rejected with 401 / 403 / 410 mid-run. Context: `request` (identity of the rejected request, `url` = actual address), `status`, `refreshCount`. Runs on the download hot path: `timeoutMs` is capped at 5 s. | `RefreshPatch`: `url`, `cookies`, `referrer`, `userAgent`, `setHeaders`, `removeHeaders`, `originHeaders` |
| `selectVariant` | HLS master playlist or DASH representations (`variants[]` with bandwidth, size, codecs). | `{ select?: index }` |
| `selectPeriod` | DASH MPD with more than one Period (`periods[]`, `manifestText`). Only the selected Period is downloaded. | `{ select?: index }` |
| `segments` | VOD playlist / MPD, once per track (`track` is `video` or `audio`). | `{ drop?: id[], rewrite?: {id, uri}[] }` |
| `liveSegments` | Each live playlist refresh (2 s ceiling on `timeoutMs`). | `{ drop?: id[], rewrite?: {id, uri}[] }` |
| `finalize` | After download, before the task is marked complete (all protocols except BT / eD2K). The task stays at "downloading 100%". Context has `filePath`, `fileName`, `saveDir`, `totalBytes`, `audioPath`. | `{ fail?, replaceWith? }` |
| `failed` | After a task fails (context: `errorMessage`, `httpStatus`, `autoRetryCount`). | `{ retry?: { delayMs? } }` |

Notes:

- `segments` / `liveSegments` receive `format`, `track`, `playlistUrl`, `manifestText` (the playlist or MPD source, up to 4 MiB; beyond that it is `""` and `manifestTextOmitted` is `true`), and `segments[]`. Each item has `id` (HLS: absolute media sequence; DASH: 0-based index among media segments, init excluded), `uri`, `duration`, `byteRange`, `discontinuity`, `title`, `programDateTime`, `dateRange`, `tags` (unrecognised tag lines verbatim), `key`, `map`, and for DASH templates `number` / `time`.
- Dropped ids from several plugins are united; later plugins only see segments not yet dropped. An unknown id, or a drop that would remove every segment, is an error for that plugin. Dropping segments invalidates a resumed download: it restarts cleanly. Dropped live segments are never re-planned.
- `rewrite` replaces a segment's address (for example a per-segment signed URL). Relative values resolve against `playlistUrl` and must end up as http(s). Later plugins see the rewritten address. A plugin's `drop` and `rewrite` are adopted together or not at all (unknown id or illegal address is an error); rewrites of dropped segments are discarded.
- `selectVariant` / `selectPeriod`: the first plugin returning a valid index wins; an out-of-range index is an error.
- `probed.fail` always fails the task, whatever `onFailure` says, as does `finalize.fail`.
- `finalize.replaceWith` is a file name inside the save directory that must already exist; the old file is removed and the task points at the new one. If the task is paused or cancelled during `finalize`, it completes with the original file.
- `failed.retry.delayMs` is clamped to `0..=300000`. The retry goes through the normal resume path, so `prepare` runs again.

## `RequestPatch`

Returned from `prepare`, and accepted as the `request` field of a [resolver](/docs/en/plugins/api-reference/) result (same shape).

| Field | Rule |
|---|---|
| `url` | At most 8 KiB, valid URL, **same host** as before, and the **protocol category must not change** (HTTP stays HTTP, HLS stays HLS …). For cross-site rewrites use a resolver. |
| `method` | HTTP tasks only; case-insensitive; `OPTIONS` rejected. |
| `body` / `clearBody` | HTTP tasks only. `body` is `{kind:"formData",fields}`, `{kind:"urlencoded",raw}` or `{kind:"raw",bytesB64,contentType?}`; `clearBody: true` removes it. |
| `cookies` | Replaces the cookie string and drops any `Cookie` header. |
| `referrer` | Empty, or an http(s) URL. Pins the Referer so the engine's own Referer inference leaves it alone. |
| `userAgent` | Non-empty, no control characters. Pins the User-Agent: the engine's anti-bot UA switching, probe-rejection adaptation and bot-challenge escape no longer change it. |
| `setHeaders` / `removeHeaders` | Names are case-insensitive. Headers the engine manages (such as `Range`, `Host`, `Content-Length`) cannot be set. Setting or removing `User-Agent` / `Referer` pins it. |
| `originHeaders` | `[{origin, headers}]` — extra headers applied only to requests whose site (`scheme://host[:port]`) equals `origin`, e.g. for segment hosts. At most 32 rules; merged per origin. |
| `segments` | `0..=512`; **overrides the user's thread count for this run** (0 = automatic). |
| `maxConnections` | `1..=512`; a hard cap that always applies. |
| `maxRangeBytes` | HTTP tasks only; integer in bytes, `>= 65536` (64 KiB). Caps the `Range` span of **every** data request of the task, for direct links that reject large ranges (for example a cloud-drive CDN that answers 403 above ~5 MiB per request — set it at or below the limit, such as `4 * 1024 * 1024`). Segment workers request their share in windows of at most this size and reuse the connection; single-stream and fallback downloads send only bounded `Range` requests of at most this size (never a plain GET or an open-ended `bytes=N-`); with `ephemeral`, the first connection carries a `Range` too. It needs a known size (`totalBytes`, or the probe) and a server that honours `Range` — otherwise it cannot be applied. The metadata probe (`HEAD`, `Range: bytes=0-0`) is unaffected. Rejected for FTP / HLS / DASH tasks. |
| `ephemeral` | HTTP tasks only. Same meaning as in a resolver result: skip the metadata probe for this run, so a one-time credential is not spent on it. |
| `totalBytes` / `rangeSupported` | HTTP only, and only together with `ephemeral: true`. `totalBytes` (> 0) is the size used instead of probing; `rangeSupported` says whether the server is known to accept ranges (default `false` on a first run). |
| `fileName` | No path separators, `..` or control characters. **Only honoured on the first run**, before the download starts; later runs ignore it and note that. |

Rules for all fields:

- **FTP** tasks accept only `url`, `segments`, `maxConnections` and `fileName`; header, cookie, method and body fields are rejected.
- A patch is **atomic**: if any field is invalid the whole patch is rejected and nothing changes. With `onFailure: "continue"` the plugin is skipped with a note.
- Proxy URL and "ignore TLS errors" are visible in `ctx.request` (`proxyUrl`, `ignoreTlsErrors`) but read-only.
- `ctx.request.userAgent` is the effective UA (header value if present, else the resolved task / queue / global UA).
- The activity log records only *which* things changed (field names, file names), never header values, cookies or bodies.

## One-time credentials

Pick the recipe by credential type:

| Credential | Recipe |
|---|---|
| Valid for a time window | `prepare` sets the header / cookie / URL query; nothing else is needed. |
| Spent by the first request | `prepare` returns the credential with `ephemeral: true` (plus `totalBytes` / `rangeSupported` if you know them) so the probe does not burn it. |
| Expires mid-download | Subscribe to `refresh`: when a request is rejected with 401 / 403 / 410 the plugin returns fresh credentials and the engine resends that request once. |
| One token per request | `ephemeral: true` in `prepare` plus `refresh` on every rejection; for streams, `rewrite` signs each segment address. |

Limits:

- Refreshing stops after **5 consecutive refreshes that never produced a successful response** (credentials that never work). A credential that succeeded at least once before being rejected counts as normally used, which resets the counter — so one-token-per-request schemes can refresh as often as needed.
- Concurrent rejections are **single-flighted**: one plugin call serves them all; a request already sent with an outdated identity is simply resent with the current one. If the plugin changes nothing, that identity generation is marked rejected and is not asked again.
- Refreshed credentials are **not persisted**; the next run starts from `prepare` again.
- Non-HTTP tasks cannot refresh `url` (other credential fields work). `refresh` cannot change `method`, `body`, `segments`, `maxConnections`, `maxRangeBytes`, `fileName` or `ephemeral`.
- `failed` retries have their own allowance of 5 and are **not** affected by the auto-retry switch.
- Once the User-Agent / Referer is pinned, probe adaptation no longer resends an identical request.

## Rewrites survive retries and are not stored

Patches are re-applied on every run, so the rewrite holds across probe retries, segment retries, anti-bot UA switches, auto-retry, manual resume, restart-resume and the HLS-to-HTTP fallback. Only the base context (the engine's own, unpinned User-Agent and Referer adaptation) is saved to the task record; plugin-added headers, the plugin's method and body, and per-site headers are **never written to the database** — the plugin supplies them again on the next run.

## Examples

Retry 403 failures after 5 seconds (stages `["failed"]`):

```js
globalThis.intercept = (ctx) =>
  ctx.stage === "failed" && ctx.httpStatus === 403 && ctx.autoRetryCount < 3
    ? { retry: { delayMs: 5000 } }
    : null;
```

Pick the highest 720p-or-lower variant (stages `["selectVariant"]`):

```js
globalThis.intercept = (ctx) => {
  const ok = ctx.variants.filter((v) => (v.height ?? 0) <= 720);
  if (!ok.length) return null;
  return { select: ok.reduce((a, b) => (b.bandwidth > a.bandwidth ? b : a)).index };
};
```
