---
title: Auth Guide
description: Let users sign in to a site from a plugin — the authenticate entry point, QR-code and password logins, and reusing credentials in resolvers.
section: plugins
order: 5
---

Many sites only serve files to a signed-in user. An **auth plugin** teaches FluxDown how to sign in to such a site. The user logs in once from the plugin's row in the settings; FluxDown stores the resulting credential and then attaches it automatically to the plugin's later requests, so your resolver never has to deal with passwords or re-login.

This guide covers the concepts, the `authenticate` entry point action by action, a complete QR-code login, password and cookie variants, and how a resolver uses the saved credential. Field-level reference for `flux.auth` and `flux.fetch` is in the [API reference](/docs/en/plugins/api-reference/); the manifest fields are in the [manifest reference](/docs/en/plugins/manifest/).

## Concepts

- **Credential store.** Credentials live in FluxDown's own store, not in your plugin's `flux.storage`. The host persists them and handles expiry, so plugins don't each reinvent that. A credential belongs to the plugin that saved it; other plugins cannot read or use it. It is deleted when the plugin is uninstalled.
- **Site.** A credential is saved for one *site*: `scheme://host[:port]`, for example `https://www.example.com`. The scheme is part of the key — `http` and `https` are different sites, and a credential for `https` is never attached to a plain `http` request. A bare `example.com` is normalized to `https://example.com`; to register a plaintext site you must write `http://example.com` yourself.
- **`authRef`.** The stable reference to a credential: `<plugin identity>::<normalized site>`, for example `acme@demo::https://www.example.com`. `flux.auth.save` returns it. You rarely build it by hand.
- **Per-site default.** If you do not pass an `authRef`, FluxDown derives the default one from your plugin identity and the URL of each request, and looks the credential up for you. This is how a saved login is reused automatically.
- **Profile.** What you save: cookies, a bearer token, basic-auth user and password and/or fixed headers, plus an optional expiry. See [`flux.auth`](/docs/en/plugins/api-reference/).

## Declare the plugin

An auth plugin needs an `auth` entry **and** the `auth` permission. A manifest that declares `auth.entry` without the permission is rejected.

```json
{
  "auth": { "entry": "auth.js", "timeoutMs": 30000 },
  "permissions": ["auth"]
}
```

| Field | Meaning |
|---|---|
| `auth.entry` | Script file that defines the global function `authenticate`. |
| `auth.timeoutMs` | Optional. Wall-clock limit of one call. Default **30 s**, ceiling 30 s; independent of the resolver's `timeoutMs`. Must not be `0`. |
| `permissions: ["auth"]` | Unlocks `flux.auth`, `ctx.authRef` in `resolve`, and the automatic credential injection into `flux.fetch`. Plugins without it never get credentials and an explicit `authRef` is refused with `插件未声明 auth 权限，不能使用认证凭据` (“the plugin does not declare the auth permission, so it cannot use credentials”). |

The `auth` permission is *elevated* risk: the user is asked to approve it when the plugin is installed. See [Permissions](/docs/en/plugins/permissions/).

## How users sign in

A plugin with an `auth.entry` gets a **Log in** button (`登录`) on its row in Settings → Extensions → Plugins, in both the desktop app and the Web UI. It opens a dialog with two optional fields:

- **Site** — a URL, passed to you as `ctx.site`. Leave it empty if your plugin has a fixed site.
- **Login input** — free text passed as `ctx.input` (an account, a verification code, …). It is a plain text box, not masked.

The dialog drives your plugin like this:

1. **On open** it sends `status` to find out whether the user is already signed in.
2. **Start login** sends `begin`.
3. If the reply is `pending` with a `sessionId` and `challengeType` `"qrcode"`, the dialog renders the challenge as a QR image and sends `poll` **every 2 seconds** by itself. For any other `challengeType` it shows the challenge as text (with a copy button) and a **Check status** button (`检查状态`) that sends `poll` on demand. A `data:image/…;base64,…` challenge is shown as an image.
4. When a reply is `success` **and** carries a non-empty `authRef`, the dialog shows “logged in” and offers **Log out** (`注销`), which sends `logout`.
5. Closing the dialog in any way sends `cancel` if a session was in progress.

The same actions are available without the UI: `daemon.plugin.auth` over `/rpc`, or `POST /api/v1/plugins/{identity}/auth` on the management API, with body `{ "action", "site", "authRef", "sessionId", "input" }`. That is how you log in on a headless server, or script a test. A plugin that is disabled cannot log in: the call fails with `插件 <identity> 未启用` (“plugin is not enabled”).

## Custom login UI

By default the **Log in** button opens the fixed dialog described above. A plugin can replace the whole dialog content with its own view by declaring a view with `location: "auth"`:

```json
{
  "permissions": ["auth"],
  "auth": { "entry": "auth.js" },
  "main": { "entry": "main.js" },
  "contributes": {
    "views": [{ "id": "login", "title": "Log in", "location": "auth" }]
  }
}
```

- At most one `auth` view per plugin, and it requires the top-level `auth` declaration (and therefore the `auth` permission). The view is portable (rendered from `main` with `flux.ui.view`) or a [desktop script](/docs/en/plugins/desktop-ui-guide/); the desktop app prefers the desktop script and the Web UI shows portable views only.
- When the plugin is enabled, loaded and has no pending consent, the dialog hosts only your view. The title stays "Log in <plugin>"; the site / login-info form and the fixed buttons are not shown. Otherwise, and in the mobile apps, the default dialog is used. The desktop app also falls back to the default dialog when the view only has a desktop script and the plugin package is not on this machine (a remote host). The button is still shown whenever the plugin has `auth.entry`.
- Drive the login from the view: call your own `flux.rpc.handle` handlers or run the flow in `main`, store the result with `flux.auth.save`, then call `ctx.close()` to dismiss the dialog (desktop scripts: `fluxdown.close()`). If the user is already logged in you can call `ctx.close()` straight from `init`.
- Apps that predate this location reject the manifest, so set `minAppVersion` to the next release or later.

```js
// main.js — QR login drawn by the plugin
const { h } = flux.ui;

flux.ui.view("login", {
  async init(ctx) {
    const res = await flux.fetch({ url: "https://passport.example.com/qr/generate" });
    const data = JSON.parse(res.body); // { key, url, image: "data:image/png;base64,…" }
    const state = { ctx, key: data.key, image: data.image, message: "Scan with the Example app", timer: 0 };
    state.timer = flux.timers.setInterval(async () => {
      const poll = JSON.parse((await flux.fetch({ url: "https://passport.example.com/qr/poll?key=" + encodeURIComponent(state.key) })).body);
      if (poll.state === "scanned") state.message = "Scanned, confirm on your phone";
      if (poll.state === "expired") state.message = "The QR code expired, reopen this dialog";
      if (poll.state === "confirmed") {
        flux.timers.clear(state.timer);
        await flux.auth.save({ site: "https://www.example.com", kind: "cookie", cookies: poll.cookies });
        state.ctx.close();
        return;
      }
      state.ctx.refresh();
    }, 2000);
    return state;
  },
  render: (state) =>
    h("stack", { gap: "md", padding: "md", align: "center" },
      h("image", { src: state.image, alt: "QR code", width: 200, height: 200 }),
      h("text", { text: state.message })),
  dispose(state) {
    flux.timers.clear(state.timer);
  },
});
```

The example assumes the login service returns the QR image and the confirmed cookies directly; adapt the requests to your site (cookies from `Set-Cookie` are read as in the [complete example](#complete-example-qr-code-login)). Without an `auth` view nothing changes: the default form dialog and the `authenticate` actions behave exactly as before.

## The `authenticate` entry point

`authenticate(ctx)` is one global function that handles all five actions. **Every call runs in a fresh script context**, so nothing survives between `begin` and `poll` unless you store it in `flux.storage` (keyed by the `sessionId` you hand out).

`ctx`:

| Field | Meaning |
|---|---|
| `action` | `begin`, `poll`, `cancel`, `logout` or `status`. |
| `site` | The site the user typed, or `""`. |
| `authRef` | The credential reference. If the caller sent none but a valid `site`, the host fills in `<identity>::<normalized site>`. For `poll`, `cancel` and `logout` the dialog sends back the `authRef` your last reply carried. |
| `sessionId` | The session identifier you returned from `begin` (sent back on `poll` and `cancel`). |
| `input` | The “Login input” text. |

Return an **object** (not a JSON string; the engine serializes it). Returning nothing is an error.

| Field | Meaning |
|---|---|
| `status` | Required: `pending`, `success` or `error`. Anything else fails with `authenticate.status 必须是 pending/success/error` (“status must be pending/success/error”). |
| `sessionId` | Your session identifier. The dialog considers a login *in progress* only when `status` is `pending` **and** `sessionId` is non-empty. |
| `challenge` | Text for the user: QR content, a URL, a `data:image/…;base64,…` image, instructions. |
| `challengeType` | `"qrcode"` makes the dialog render the challenge as a QR code and poll automatically. Anything else is shown as text. |
| `message` | Status text. Shown in error style only when `status` is `error`. |
| `authRef` | The credential `flux.auth.save` returned. If you return `success` without it, the host copies the request's `authRef` into the reply. |

While `pending`, a `poll` reply that leaves out `challenge`/`challengeType` keeps the previous challenge on screen. Any other reply clears fields you leave out; an explicit empty string removes the challenge. QR challenges that are ordinary text (including login URLs) are rendered into a QR image locally; the URL is never fetched as an image.

### What each action should do

| Action | You receive | You should | Reply |
|---|---|---|---|
| `begin` | `site`, `input`, maybe `authRef` | Start the flow. Create a session and remember it. For a one-step login (password, pasted cookie) finish right here. | `pending` + `sessionId` + `challenge` for interactive flows; `success` + `authRef` for one-step logins; `error` + `message` if it cannot start. |
| `poll` | `sessionId`, `authRef`, `input` | Ask the platform whether the user has finished. On success call `flux.auth.save`. | `pending` while waiting (keep `sessionId`); `success` + `authRef` when done; `error` if it expired or was refused. |
| `cancel` | `sessionId` | Throw the session away (clean up your storage). Best effort; the dialog ignores the reply. | Any valid status; `error` with “cancelled” is natural. |
| `logout` | `authRef` (or `site`) | Optionally tell the platform to end the session. **You do not need to delete the credential.** | `success`. |
| `status` | optionally `site`, `authRef` | Report the current state without side effects. | See below. |

### The three `status` outcomes

`status` is how the dialog (and any API client) learns whether the user is signed in. The dialogs treat the user as logged in only for `success` **with a non-empty `authRef`**, so return exactly one of:

| Situation | Reply |
|---|---|
| Signed in and the credential works | `{ status: "success", authRef, message }` |
| Not signed in | `{ status: "pending", message: "Not logged in" }` — no `sessionId`, so the dialog just offers **Start login** and shows the message in neutral style. |
| A credential exists but is expired or rejected | `{ status: "error", message: "Session expired, please log in again" }` |

## Complete example: QR-code login

A fictional site `www.example.com` whose login service `passport.example.com` offers three endpoints (this is the usual shape of QR logins):

- `GET /qr/generate` → `{ "key": "…", "url": "https://…" }` — the text to put into a QR code.
- `GET /qr/poll?key=…` → `{ "state": "waiting" | "scanned" | "confirmed" | "expired" }`. On `confirmed` the response carries `Set-Cookie` headers.
- `GET https://www.example.com/api/me` → `{ "name": "…" }`, or HTTP 401 when the cookies are missing or invalid.

`manifest.json`

```json
{
  "identity": "acme@example-login",
  "name": "Example Site Login",
  "version": "1.0.0",
  "description": "Signs in to www.example.com with a QR code and resolves its video pages.",
  "resolvers": [
    {
      "match": { "urls": ["*://www.example.com/video/*"] },
      "entry": "resolve.js"
    }
  ],
  "auth": { "entry": "auth.js" },
  "permissions": ["auth"]
}
```

`auth.js`

```js
const PASSPORT = "https://passport.example.com";
const SITE = "https://www.example.com";
const THIRTY_DAYS = 30 * 24 * 3600;

// Several Set-Cookie headers arrive as ONE string, joined with "\n". Keep only
// the "name=value" part of each line to build a request Cookie header.
function cookieHeader(setCookie) {
  return String(setCookie || "")
    .split("\n")
    .map((line) => line.split(";")[0].trim())
    .filter(Boolean)
    .join("; ");
}

function savedAuthRef(ctx) {
  // Canonical form: "<plugin identity>::<normalized site>".
  return ctx.authRef || flux.info.identity + "::" + SITE;
}

async function loadSession() {
  const raw = await flux.storage.get("session");
  return raw ? JSON.parse(raw) : null;
}

async function begin() {
  const res = await flux.fetch({ url: PASSPORT + "/qr/generate" });
  if (res.status !== 200) {
    return { status: "error", message: "QR service answered HTTP " + res.status };
  }
  const data = JSON.parse(res.body);
  // Each call runs in a fresh context: persist what poll will need.
  const sessionId = String(Date.now());
  await flux.storage.set("session", JSON.stringify({ sessionId, key: data.key }));
  return {
    status: "pending",
    sessionId,
    challenge: data.url,
    challengeType: "qrcode",
    message: "Scan the code with the Example app",
  };
}

async function poll(ctx) {
  const session = await loadSession();
  if (!session || session.sessionId !== ctx.sessionId) {
    return { status: "error", message: "The login session is gone, start again" };
  }

  const res = await flux.fetch({
    url: PASSPORT + "/qr/poll?key=" + encodeURIComponent(session.key),
  });
  const data = JSON.parse(res.body);

  if (data.state === "waiting") {
    // No challenge fields: the dialog keeps showing the QR code.
    return { status: "pending", sessionId: ctx.sessionId };
  }
  if (data.state === "scanned") {
    return { status: "pending", sessionId: ctx.sessionId, message: "Scanned, confirm on your phone" };
  }
  if (data.state !== "confirmed") {
    await flux.storage.set("session", "");
    return { status: "error", message: "The QR code expired" };
  }

  const cookies = cookieHeader(res.headers["set-cookie"]);
  const me = await flux.fetch({ url: SITE + "/api/me", headers: { Cookie: cookies } });
  const name = me.status === 200 ? JSON.parse(me.body).name : "";

  const authRef = await flux.auth.save({
    site: SITE,
    kind: "cookie",
    account: name,
    cookies,
    expiresAt: Math.floor(Date.now() / 1000) + THIRTY_DAYS,
  });
  await flux.storage.set("session", "");
  return { status: "success", authRef, message: name ? "Logged in as " + name : "Logged in" };
}

async function cancel() {
  await flux.storage.set("session", "");
  return { status: "error", message: "Login cancelled" };
}

async function logout() {
  // Optional courtesy call. The host deletes the credential afterwards anyway.
  try {
    await flux.fetch({ method: "POST", url: SITE + "/api/logout" });
  } catch (e) {
    flux.logger.warn("remote logout failed", String(e && e.message));
  }
  return { status: "success", message: "Logged out" };
}

async function status(ctx) {
  const profile = await flux.auth.get(savedAuthRef(ctx));
  if (!profile) return { status: "pending", message: "Not logged in" };

  // The request goes to the credential's own site, so the saved cookies are
  // attached automatically. An expired credential is skipped, which yields 401.
  const me = await flux.fetch({ url: SITE + "/api/me" });
  if (me.status !== 200) {
    return { status: "error", message: "Session expired, please log in again" };
  }
  return {
    status: "success",
    authRef: profile.authRef,
    message: "Logged in as " + JSON.parse(me.body).name,
  };
}

async function authenticate(ctx) {
  switch (ctx.action) {
    case "begin":
      return begin();
    case "poll":
      return poll(ctx);
    case "cancel":
      return cancel();
    case "logout":
      return logout();
    case "status":
      return status(ctx);
    default:
      return { status: "error", message: "Unsupported action " + ctx.action };
  }
}
```

`resolve.js`

```js
async function resolve(ctx) {
  let res;
  try {
    // ctx.authRef is the default credential for this URL's site ("" when not
    // logged in or when the permission is missing). Passing it makes a missing
    // login a hard error instead of an anonymous request.
    res = await flux.fetch({
      url: "https://www.example.com/api/play?u=" + encodeURIComponent(ctx.url),
      authRef: ctx.authRef || undefined,
    });
  } catch (e) {
    // The message carries a prefix such as "插件运行时错误: ", so use includes().
    if (String(e && e.message).includes("authentication_required")) {
      throw new Error("Please log in first: Settings → Extensions → Plugins → Log in");
    }
    throw e;
  }
  if (res.status === 401) throw new Error("Login expired, log in again");
  if (res.status !== 200) throw new Error("API answered HTTP " + res.status);

  const data = JSON.parse(res.body);
  return { url: data.url, fileName: data.fileName, ephemeral: true };
}
```

Points worth copying:

- **Session state in storage.** `sessionId` ties a `poll` to the `begin` that created it; a stale dialog cannot complete someone else's session. Use one storage key, not one per session, to stay inside the 100-key limit.
- **Cookies from a login response.** `flux.fetch` joins repeated headers with `"\n"` and lower-cases header names, hence `res.headers["set-cookie"]` and the `split("\n")`.
- **Return the `authRef`** from `flux.auth.save`; the dialog needs it to show the logged-in state.
- **`logout` is simple.** The host removes the credential for the `authRef` it sent, after your script returns — even if your script threw. You never need to call `flux.auth.remove` for the normal logout.

## Variants without a challenge

When there is nothing to scan, finish in `begin` and return `success` straight away.

### Username and password

The user types `name:password` into **Login input**:

```js
async function begin(ctx) {
  const i = ctx.input.indexOf(":");
  if (i <= 0) return { status: "error", message: "Enter the login as username:password" };
  const user = ctx.input.slice(0, i);
  const pass = ctx.input.slice(i + 1);

  const res = await flux.fetch({
    method: "POST",
    url: SITE + "/api/login",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: "username=" + encodeURIComponent(user) + "&password=" + encodeURIComponent(pass),
  });
  if (res.status !== 200) return { status: "error", message: "Login rejected (HTTP " + res.status + ")" };

  const authRef = await flux.auth.save({
    site: SITE,
    kind: "cookie",
    account: user,
    cookies: cookieHeader(res.headers["set-cookie"]),
  });
  return { status: "success", authRef, message: "Logged in as " + user };
}
```

Store the *session cookie or token* the site gives back, not the password, whenever you can. For a site that really uses HTTP Basic, save `{ site, kind: "basic", username, password }`: FluxDown then sends `Authorization: Basic …` for you.

### Pasted cookie or token

The user copies a cookie string (or API token) from their browser into **Login input**:

```js
async function begin(ctx) {
  const cookies = ctx.input.trim();
  if (!cookies) return { status: "error", message: "Paste your cookie string" };

  const me = await flux.fetch({ url: SITE + "/api/me", headers: { Cookie: cookies } });
  if (me.status !== 200) return { status: "error", message: "The cookie was rejected (HTTP " + me.status + ")" };

  const authRef = await flux.auth.save({ site: SITE, kind: "cookie", cookies });
  return { status: "success", authRef };
}
```

For a bearer token use `{ site, kind: "bearer", accessToken }`; FluxDown adds `Authorization: Bearer …`. For a token that never changes and needs no login dialog, a `password`-widget [setting](/docs/en/plugins/manifest/) is simpler than an auth plugin.

## Saving credentials

`flux.auth.save(profile)` returns the `authRef` string.

| Field | Meaning |
|---|---|
| `site` | **Required.** Full URL or `host[:port]`; normalized as described above. An unusable value fails with `认证档案的 site 必须是有效的 URL 或 host` (“the profile's site must be a valid URL or host”); a missing one with `认证档案必须提供 site`. |
| `authRef` | Leave out. If given it must equal `<identity>::<normalized site>`, otherwise `authRef 必须等于当前插件和规范化站点组成的引用` (“authRef must equal the reference built from this plugin and the normalized site”). |
| `kind` | `cookie`, `bearer`, `basic`, `headers` or `session`. Only `basic` changes behaviour (it enables Basic auth from `username`/`password`); the others are labels. |
| `cookies` | Injected as the `Cookie` header (unless the request already has one). |
| `accessToken` | Injected as `Authorization: Bearer …` (unless `Authorization` is set). |
| `username`, `password` | Used with `kind: "basic"`. |
| `headers` | Extra headers; they win over the ones generated from the fields above. |
| `expiresAt` | Unix **seconds**, an integer. After it the credential counts as expired. Leave out for cookies whose lifetime the server decides. |
| `account`, `refreshToken`, `refreshAt`, `metadata` | Yours; stored, not interpreted (`metadata` is a string-to-string map). |

A profile larger than 64 KB is refused (`认证档案超过 65536 字节上限`, “the profile exceeds the 65536-byte limit”). `flux.auth.get(authRef)` returns the stored profile or `null`; `flux.auth.remove(authRef)` resolves to `true`. Both refuse an `authRef` that belongs to another plugin (`认证凭据不属于当前插件` / `authRef 必须属于当前插件`).

## Using credentials in requests

With the `auth` permission, `flux.fetch` looks credentials up for every request:

- **No `authRef` (implicit).** The default reference is built from your identity and the request's site. If a valid credential exists it is attached; if there is none, or it has expired, the request goes out **without** credentials and no error is raised.
- **Explicit `authRef`** (usually `ctx.authRef`). A missing credential rejects with `authentication_required: 未找到认证凭据 <authRef>` (“credential not found”); an expired one with `authentication_required: 认证凭据已过期 <authRef>` (“credential expired”). Thrown errors may be wrapped in a prefix like `插件运行时错误: `, so test with `includes("authentication_required")`, not `startsWith`.

Rules that surprise people:

1. **The site must match.** A credential is only attached to a request whose scheme, host and port equal the credential's `site`. An explicit `authRef` used against another site is refused: `认证凭据不属于当前请求站点` (“the credential does not belong to the requested site”). If your resolver talks to several hosts (`www.example.com`, `api.example.com`), save one credential per host.
2. **`ctx.authRef` follows the task URL.** In `resolve`, `ctx.authRef` is the default reference for the *task URL's* site, not for the API host you call. Use it with requests to that same site, or leave `authRef` out and let the host derive it per request.
3. **No cross-origin redirects.** When credentials were attached, a redirect to a different scheme, host or port is **not followed**: you receive the 3xx response itself (with its `location` header). Follow it yourself if you accept sending no credentials, or fix the URL (for instance `http` → `https`).
4. **Explicit headers win.** An existing `Cookie` or `Authorization` header on the request is never overwritten by the credential's cookies or token.
5. **Hooks and subscriptions** can use credentials the same way: `flux.auth` and the injection also work in hooks and `subscribe` when the permission is declared, but there is no `ctx.authRef` there — rely on the implicit default.

## What the host does for you

- **Logout always cleans up.** After a `logout` call returns — or throws — the host deletes the credential for the requested `authRef`. If the plugin is **disabled**, logout does not even run your script: the host removes the credential directly, so users can always sign out of a broken plugin.
- **No circuit breaker.** Timeouts and crashes in `authenticate` do not count towards the auto-disable that guards resolvers.
- **Own budget and pool.** One call may run up to `auth.timeoutMs` (default and ceiling 30 s), independent of the resolver. Auth calls use a small dedicated pool (two at once), so login polling never starves downloads; if the pool stays full for 3 s the call fails with `插件运行时过载（并发已满）` (“the plugin runtime is overloaded”).
- **Required settings are enforced.** A `required` setting without a value fails the call with `缺少必填设置项: 插件 <identity> 需先配置「<title>」` (“missing required setting: the plugin needs <title> configured first”).
- **Validation.** `action` must be one of the five (`auth action 必须是 begin/poll/cancel/logout/status`); a `logout` without `authRef` and `site` fails with `logout 需要 authRef 或 site`.

## Errors

| Message | Cause |
|---|---|
| `authenticate 返回值非法: …` (“authenticate returned an invalid value”) | You returned nothing, a non-object, or a value that does not parse. Return a plain object. |
| `authenticate.status 必须是 pending/success/error` | `status` missing or misspelled. |
| `入口 authenticate 未定义` (“entry authenticate is not defined”) | The script does not define a global `authenticate` (a `const` arrow function does not count — use a `function` declaration). |
| `插件 <identity> 未声明 auth 权限` / `插件 <identity> 未提供 auth 入口` | The permission or `auth.entry` is missing from the manifest. |
| `脚本执行超时` (“script execution timed out”) | The call exceeded `auth.timeoutMs`. |
| `authentication_required: …` | `flux.fetch` with an explicit `authRef` and no valid credential. |
| `认证凭据不属于当前插件` (“the credential does not belong to this plugin”) | An `authRef` of another plugin. |
| The dialog shows “Login failed: …” (`登录失败：…`) | Any of the above, wrapped; the text after the colon is the engine message. |
| The dialog shows “The plugin returned an invalid login response” | The reply carried no string `status`. |

Host and engine messages are Chinese; they are quoted here exactly as you will see them in the dialog and the log.

## Testing tips

1. Install the folder in dev mode and approve the `auth` permission. `auth.js` is re-read on every call; manifest changes need the **Reload** button (`重新加载`) on the plugin row.
2. Open **Log in**. Use `flux.logger.info` liberally and read the plugin log ([Debugging](/docs/en/plugins/debugging/)).
3. After a successful login, add a download from the matching site and check the resolver log for the request you made.
4. To exercise one action at a time, call the management endpoint: `curl -X POST -H "Authorization: Bearer <token>" -d '{"action":"status"}' http://127.0.0.1:17800/api/v1/plugins/acme@example-login/auth` (the management API must be enabled; see the [API overview](/docs/en/api/overview/)).
5. Test expiry by saving a profile with `expiresAt` a few seconds in the future.

Next: [Hooks guide](/docs/en/plugins/hooks-guide/) for reacting to download events (hooks can use `flux.auth` too), and [Your first plugin](/docs/en/plugins/your-first-plugin/) for the resolver basics.
