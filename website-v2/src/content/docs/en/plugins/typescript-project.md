---
title: TypeScript Project
description: Build a complete plugin with fluxdown-plugin-sdk and TypeScript — resolver, hook, settings, a JSX view and a command — then validate and pack it into a .fxplug.
section: plugins
order: 3
---

This tutorial takes you from an empty folder to a packed `.fxplug` using [`fluxdown-plugin-sdk`](/docs/en/plugins/sdk/). You will build **Mirror Demo**, a small plugin that does five things:

| Feature | Where it lives |
|---|---|
| a **resolver** that rewrites `https://mirror-demo.invalid/files/<name>` to a configurable mirror | `src/resolve.ts` |
| a **hook** that counts finished downloads | `src/hooks.ts` |
| a **settings** form with the mirror URL | `manifest.json` → `settings` |
| a **view** on the activity bar and a **command** that adds a sample download | `src/main.tsx` |
| the `tasks` **permission** the view needs to create downloads | `manifest.json` → `permissions` |

You need Node.js 20 or newer, and FluxDown (desktop app, or the Web UI of a server you control). The tutorial assumes you have read nothing else, but [Your first plugin](/docs/en/plugins/your-first-plugin/) explains the resolver concepts in more depth, and the [overview](/docs/en/plugins/overview/) explains the execution model.

## 1. Create the project

```bash
npx fluxdown-plugin-sdk create mirror-demo --identity me@mirror-demo --name "Mirror Demo"
cd mirror-demo
npm install
```

- `--identity` is the permanent plugin ID, `author@name` with lowercase letters, digits, `_` and `-`. If you omit it, it is `me@` plus the folder name; `--name` defaults to the folder name.
- `create` refuses to write into a non-empty folder.
- The generated `package.json` depends on `fluxdown-plugin-sdk` and `typescript`; `npm install` fetches both.

## 2. Tour of the generated files

```
mirror-demo/
├── manifest.json                  what the plugin is and contributes
├── fluxdown-plugin.config.json    which source files are bundled into which script
├── package.json                   scripts: build, validate, pack, dev, typecheck
├── tsconfig.json                  strict, ES2022, JSX from the SDK, no DOM
├── .gitignore                     node_modules/, dist/, *.fxplug
└── src/
    ├── env.d.ts                   makes the global `flux` known to TypeScript
    └── main.tsx                   a counter view and a "Say hello" command
```

- **`manifest.json`** is the source of truth. It declares a resident `main` script (`main.js`), one activity-bar view `dashboard` and one palette command `hello`. The [manifest reference](/docs/en/plugins/manifest/) lists every field.
- **`fluxdown-plugin.config.json`** maps *output script in `dist/`* to *source file*: `"main.js": "src/main.tsx"`. Every script the manifest names must either be listed here or exist as a plain file in the project root.
- **`src/main.tsx`** runs in the resident `main` context. `defineMain((flux) => …)` wraps the registration code: `flux.ui.view("dashboard", …)` registers the view whose `id` is declared in the manifest, and `flux.commands.register("hello", …)` registers the command. The JSX tags (`<stack>`, `<button>`, …) are view nodes; the [SDK reference](/docs/en/plugins/sdk/#jsx) lists the rules.
- **`dist/`** does not exist yet. It is the build output — the folder FluxDown actually installs.

## 3. Build once and install it in FluxDown

```bash
npm run typecheck     # tsc --noEmit: type errors only, no files written
npm run build         # bundles src/ into dist/, copies manifest.json, validates
npm run validate      # prints: valid (risk: standard)
```

`dist/` now contains `main.js` and `manifest.json`. Install it as a **dev plugin**:

1. Open **Settings → Extensions → Plugins**.
2. Under **Install from directory (dev mode)** choose the **`dist/`** folder (not the project root) and press **Install**. In the Web UI, type the absolute path of `dist/` **on the machine that runs FluxDown**; for a server or Docker container that means a path inside it (for example a bind mount).

A dev plugin stores the folder path instead of copying the files, shows a **Dev** badge and has a **Reload** button (tooltip: “Reload (re-read manifest and scripts)”). The plugin has no permissions, so it is enabled right away. You should see a **Mirror Demo** entry on the activity bar with a click counter, and **Say hello** in the command palette.

Two rules of the dev loop, both important when `dist/` is the installed folder:

- **`npm run build`, `npm run pack` and `npm run dev` delete `dist/` before they rebuild.** For a moment the installed folder is empty; let the build finish. If the plugin row shows **Load failed** after a rebuild, press **Reload** once the build is done.
- **Manifest and `main` changes need the Reload button.** Resolver, hooks, auth and subscription scripts of a dev plugin are re-read on every call, so for those a rebuild is enough.

From now on keep the watcher running in a second terminal:

```bash
npm run dev
```

It rebuilds `dist/` on every save and prints `rebuilt, valid` (or the list of problems). It reads `fluxdown-plugin.config.json` only once at start, so **restart it after you edit the config**.

## 4. Add a resolver

A resolver rewrites a task's URL right before it starts. Ours rewrites `https://mirror-demo.invalid/files/<name>` to `<mirror>/<name>`.

**Declare it.** In `manifest.json` add `resolvers` and a `settings` entry for the mirror URL:

```json
{
  "resolvers": [
    {
      "match": { "urls": ["*://mirror-demo.invalid/files/*"] },
      "entry": "resolve.js"
    }
  ],
  "settings": [
    {
      "key": "mirror",
      "title": "Mirror base URL",
      "description": "Links under mirror-demo.invalid/files/ are downloaded from here instead.",
      "type": "string",
      "default": "https://proof.ovh.net/files",
      "pattern": "^https://"
    }
  ]
}
```

(Add these two keys next to the existing ones; the complete manifest is shown in step 6.) `match.urls` uses `*` as its only wildcard against the whole URL; `pattern` is a JavaScript regular expression that the settings form uses to validate the input.

**Bundle it.** Add the entry to `fluxdown-plugin.config.json`:

```json
{
  "entries": {
    "main.js": "src/main.tsx",
    "resolve.js": "src/resolve.ts"
  },
  "desktop": {},
  "assets": []
}
```

**Write it.** Create `src/resolve.ts`:

```ts
import { defineResolver } from "fluxdown-plugin-sdk/runtime";

const MARKER = "/files/";

defineResolver((ctx) => {
  const start = ctx.url.toLowerCase().indexOf(MARKER);
  if (start < 0) return null; // not ours: download the original URL

  // Everything after /files/, without query string or fragment.
  const name = ctx.url.slice(start + MARKER.length).split(/[?#]/)[0];
  if (!name) return null;

  const mirror = String(flux.settings.mirror).replace(/\/+$/, "");
  const url = `${mirror}/${name}`;
  flux.logger.info("rewriting", ctx.url, "->", url);

  return { url, fileName: name };
});
```

What to notice:

- `defineResolver` installs your function as the global `resolve` that the engine calls. The function may be `async`; here it needs no `await`.
- `ctx` is typed as `ResolveContext`; the result type is `ResolveResult`. Returning `null` means “not mine”. Throwing makes the task fail visibly (fail-closed).
- `flux.settings.mirror` is typed `string | number | boolean | null`, so it is wrapped in `String(...)`. Because the setting has a `default`, it always has a value.
- The plugin JavaScript environment is plain QuickJS — there is **no `URL` class** and no `setTimeout`, which is why the code slices the string. Because `lib` is `ES2022` without DOM, TypeScript would flag `new URL(...)` for you.
- Everything runs **inside** the function: `flux` does not exist while FluxDown compiles the script during install, so top-level `flux.*` calls would break the install.

**Try it.** Restart `npm run dev` (the config changed), wait for `rebuilt, valid`, then press **Reload** on the plugin row. Then add a download for `https://mirror-demo.invalid/files/1Mb.dat`. The host does not exist, and does not need to: the resolver rewrites the link before any connection is made. The task downloads `1Mb.dat` from the mirror (the default mirror is a public test-file host used by FluxDown's own demo plugin; change **Mirror base URL** under the plugin's settings (gear button) if you prefer another). The log shows:

```
… [plugin:me@mirror-demo] rewriting https://mirror-demo.invalid/files/1Mb.dat -> https://proof.ovh.net/files/1Mb.dat
```

If nothing happens, see [Debugging](/docs/en/plugins/debugging/) — the first suspects are the URL pattern and a forgotten **Reload**.

## 5. Add a hook

A hook is notified when a task changes state. Ours counts completed downloads and remembers the last file, using `flux.storage` — the only state that survives between calls, because every hook call runs in a fresh JavaScript context.

**Declare it** in `manifest.json`:

```json
{
  "hooks": {
    "entry": "hooks.js",
    "events": ["onDone"]
  }
}
```

`events` lists which of `onStart`, `onError`, `onDone`, `onMetaProbed`, `onCancel` the plugin wants. Without `hooks.match` the hook fires for every task.

**Bundle it.** Add `"hooks.js": "src/hooks.ts"` to `entries` in `fluxdown-plugin.config.json`, then restart `npm run dev`.

**Write it.** Create `src/shared.ts` — a normal module shared by the hook and the view, which is what bundling is for:

```ts
/** Storage keys shared by the hook (writer) and the view (reader). */
export const DONE_COUNT_KEY = "doneCount";
export const LAST_FILE_KEY = "lastFile";
```

and `src/hooks.ts`:

```ts
import { defineHooks } from "fluxdown-plugin-sdk/runtime";
import { DONE_COUNT_KEY, LAST_FILE_KEY } from "./shared";

defineHooks({
  async onDone(ev) {
    const previous = Number((await flux.storage.get(DONE_COUNT_KEY)) ?? "0");
    await flux.storage.set(DONE_COUNT_KEY, String(previous + 1));
    await flux.storage.set(LAST_FILE_KEY, ev.filePath);
    flux.logger.info("finished", ev.taskId, ev.filePath);
  },
});
```

`ev` is typed from the event: `taskId`, `url`, `filePath`, `audioPath` and `muxed` for `onDone`. Storage values are strings (`null` when unset). A hook that throws or times out only writes a warning to the log; it can never break a download. [Hooks guide](/docs/en/plugins/hooks-guide/) covers retries, ffmpeg and the other events.

**Try it.** After **Reload**, let any download finish. The log shows `[plugin:me@mirror-demo] finished <task id> <file path>`.

## 6. Add a view, a command and a permission

The resident `main` script is where views and commands live. Ours shows the settings and statistics, and has a button that creates a download through `flux.tasks`. That API is guarded by a permission, so we declare it and let FluxDown ask the user.

Final `src/shared.ts` — add the sample URL (the resolver will rewrite it):

```ts
/** Storage keys shared by the hook (writer) and the view (reader). */
export const DONE_COUNT_KEY = "doneCount";
export const LAST_FILE_KEY = "lastFile";

/** A link the resolver rewrites; the host name does not need to exist. */
export const SAMPLE_URL = "https://mirror-demo.invalid/files/1Mb.dat";
```

Replace `src/main.tsx`:

```tsx
import { defineMain } from "fluxdown-plugin-sdk/runtime";
import type { MainFlux, ViewEvent } from "fluxdown-plugin-sdk";
import { DONE_COUNT_KEY, LAST_FILE_KEY, SAMPLE_URL } from "./shared";

interface State {
  doneCount: number;
  lastFile: string;
  note: string;
}

async function loadStats(): Promise<Pick<State, "doneCount" | "lastFile">> {
  return {
    doneCount: Number((await flux.storage.get(DONE_COUNT_KEY)) ?? "0"),
    lastFile: (await flux.storage.get(LAST_FILE_KEY)) ?? "",
  };
}

async function downloadSample(flux: MainFlux): Promise<void> {
  if (!flux.tasks) throw new Error('declare the "tasks" permission in manifest.json');
  await flux.tasks.create({ request: { url: SAMPLE_URL } });
}

defineMain((flux) => {
  flux.ui.view<State>("dashboard", {
    init: async () => ({ ...(await loadStats()), note: "" }),

    render: (state) => (
      <stack gap="md" padding="md">
        <heading text="Mirror Demo" />
        <callout
          tone="info"
          text="Links under mirror-demo.invalid/files/ are downloaded from the mirror set in this plugin's settings."
        />
        <kv
          items={[
            { key: "Mirror", value: String(flux.settings.mirror) },
            { key: "Completed downloads", value: String(state.doneCount) },
            { key: "Last file", value: state.lastFile || "-" },
          ]}
        />
        <stack key="actions" direction="row" gap="sm">
          <button key="sample" text="Download sample file" variant="primary" />
          <button key="refresh" text="Refresh" />
        </stack>
        {state.note ? <text text={state.note} tone="muted" /> : null}
      </stack>
    ),

    async event(state, ev: ViewEvent) {
      if (ev.type !== "press") return;
      switch (ev.node) {
        case "root/actions/sample":
          try {
            await downloadSample(flux);
            state.note = "Sample download added.";
          } catch (error) {
            state.note = `Could not add the download: ${String(error)}`;
          }
          break;
        case "root/actions/refresh":
          Object.assign(state, await loadStats(), { note: "" });
          break;
      }
    },
  });

  flux.commands.register("download-sample", async () => {
    await downloadSample(flux);
    flux.ui.toast("success", "Sample download added");
  });
});
```

How it works:

- **`init`** builds the initial state when the view opens; **`render(state)`** turns state into a node tree and must have no side effects; **`event(state, ev)`** mutates `state` in place and FluxDown re-renders afterwards.
- **Node ids.** The root node is `root`; a child's id is its `key`, or its position when it has none. The row `<stack key="actions">` contains the buttons keyed `sample` and `refresh`, so their ids are `root/actions/sample` and `root/actions/refresh` — that is what `ev.node` carries. Keys must be unique among siblings. The [UI guide](/docs/en/plugins/ui-guide/) goes deeper into events, controlled inputs and timers.
- **JSX children are nodes only.** There is no `<button>Text</button>`: text goes into the `text` prop, and free text needs a `<text>` node. `null` and `false` children are dropped, which is why `{cond ? <text …/> : null}` works.
- **`defineMain` hands you a `MainFlux`**: the global `flux` type is shared by every entry script, so `ui`, `commands`, `rpc`, `events` and `timers` are only typed on the `flux` that `defineMain` passes in. `flux.tasks` is optional in the types because it only exists when the manifest declares the `tasks` permission. The guard turns a missing declaration into a readable message instead of a `TypeError`.
- **`flux.commands.register`** ties a handler to the manifest command with the same `id`; `flux.ui.toast` shows a notification.
- `flux.settings` in `main` reflects the saved settings, and `flux.storage` is the same store the hook writes to.

Now update the manifest. This is the **complete** `manifest.json` at this point:

```json
{
  "identity": "me@mirror-demo",
  "name": "Mirror Demo",
  "version": "0.1.0",
  "description": "Rewrites mirror-demo.invalid links to a configurable mirror and shows download stats.",
  "resolvers": [
    {
      "match": { "urls": ["*://mirror-demo.invalid/files/*"] },
      "entry": "resolve.js"
    }
  ],
  "hooks": {
    "entry": "hooks.js",
    "events": ["onDone"]
  },
  "main": { "entry": "main.js" },
  "contributes": {
    "views": [
      { "id": "dashboard", "title": "Mirror Demo", "location": "activity" }
    ],
    "commands": [
      { "id": "download-sample", "title": "Mirror Demo: download sample file", "palette": true }
    ]
  },
  "settings": [
    {
      "key": "mirror",
      "title": "Mirror base URL",
      "description": "Links under mirror-demo.invalid/files/ are downloaded from here instead.",
      "type": "string",
      "default": "https://proof.ovh.net/files",
      "pattern": "^https://"
    }
  ],
  "permissions": ["tasks"]
}
```

The changes since step 3: the command `hello` became `download-sample`, and `permissions` was added. View and command titles must be 1–40 characters; ids must match `^[a-z][a-z0-9-]{0,31}$`.

**Approve the permission.** Press **Reload**. The manifest now requests `tasks`, so the plugin row switches to **Authorization required**, the enable switch is locked, and the app opens an approval dialog listing the requested capabilities (reopen it with the **Authorize** button). The dialog shows a risk badge: `tasks` is **Elevated** (only `desktop` is *Full trust*). Press **Authorize**; the plugin becomes enabled. Any later manifest that asks for *additional* permissions or network hosts goes through the same step. See [Permissions](/docs/en/plugins/permissions/) for what each permission allows and what is audited.

**Try it.**

1. Open **Mirror Demo** on the activity bar. The statistics and the mirror URL are shown.
2. Press **Download sample file**. A task for `https://mirror-demo.invalid/files/1Mb.dat` appears, is rewritten by your resolver and downloads.
3. When it finishes, press **Refresh**: *Completed downloads* has increased and *Last file* shows the path — written by your hook.
4. Open the command palette and run **Mirror Demo: download sample file**.

## 7. Typecheck, validate and pack

```bash
npm run typecheck
npm run validate      # valid (risk: elevated)
npm run pack          # packed …/me@mirror-demo-0.1.0.fxplug
```

- `typecheck` runs `tsc --noEmit` with strict settings; fix what it reports before packing.
- `validate` applies the same rules as the app and the plugin index, in strict mode. The result line includes the **risk level** derived from the permissions. Error messages are in Chinese, like the engine's; the [Debugging](/docs/en/plugins/debugging/#install-and-validation-errors) page translates the common ones.
- `pack` builds, validates and zips `dist/` into `<identity>-<version>.fxplug` in the project root, with the manifest at the archive root. The zip is deterministic, so repeated packs of the same sources give the same bytes.

To test the package the way a user would install it, first **uninstall the dev copy** (a dev registration takes precedence over an installed copy of the same identity), then use **Install .fxplug / .zip plugin** and pick the file. The authorization dialog appears again because this is a fresh install.

## 8. Where to go next

- **Ship it:** bump `version` in `manifest.json` for every release, then follow [Packaging & market](/docs/en/plugins/packaging/) to publish the `.fxplug` to the plugin index.
- **More capabilities:** [Hooks guide](/docs/en/plugins/hooks-guide/) (retries, ffmpeg, yt-dlp), [Auth guide](/docs/en/plugins/auth-guide/) (login flows), [Subscription guide](/docs/en/plugins/subscription-guide/), [UI guide](/docs/en/plugins/ui-guide/) (inputs, tables, timers, settings and task-detail tabs), [Desktop UI guide](/docs/en/plugins/desktop-ui-guide/).
- **When something is off:** [Debugging](/docs/en/plugins/debugging/).
- **Reference:** [SDK & CLI](/docs/en/plugins/sdk/), [Manifest](/docs/en/plugins/manifest/), [API](/docs/en/plugins/api-reference/), [Views](/docs/en/plugins/views/).
- **Full examples** in the repository: [`feature-demo`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk/examples/feature-demo) (every capability, with an acceptance checklist) and [`youtube-down`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk/examples/youtube-down) (a self-contained site parser, bundled third-party scripts, `memoryLimitMb`, multi-file listings, DOM sniffing rules).
