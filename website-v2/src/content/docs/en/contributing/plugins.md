---
title: Publishing Plugins
description: Build a plugin, publish it to the FluxDown plugin market from your own GitHub repository, and ship updates.
section: contributing
order: 1
---

Plugins live in **your own public GitHub repository**. The plugin market is the open index [zerx-lab/fluxdown-plugin-index](https://github.com/zerx-lab/fluxdown-plugin-index): it records which repository publishes which plugin identity and picks up every new GitHub Release automatically. You register once; after that, publishing an update is just publishing a release. Everything already published is listed in the [plugin market](/plugins/).

## 1. Build the plugin

- New to plugins? Start with the [Plugin overview](/docs/en/plugins/overview/) and [Your first plugin](/docs/en/plugins/your-first-plugin/).
- For TypeScript, several source files or views written in JSX, use the official SDK [`fluxdown-plugin-sdk`](https://www.npmjs.com/package/fluxdown-plugin-sdk):

  ```bash
  npx fluxdown-plugin-sdk create my-plugin --identity me@my-plugin --name "My Plugin"
  ```

  It bundles, validates and packs a `.fxplug` with the same manifest validator the engine and the index use. See [Plugin SDK & CLI](/docs/en/plugins/sdk/).
- Declare only the permissions you need (see [Permissions](/docs/en/plugins/permissions/)). Users see them before installing, and the index bot lists them on your registration pull request.

## 2. Publish a release

Check the package with `fluxdown-plugin validate` first. Then publish a GitHub Release (not a draft, not a pre-release) with **exactly one** `.fxplug` asset; it must be the repository's *Latest* release. `version` in `manifest.json` must be plain `MAJOR.MINOR.PATCH` and higher than every version already in the index.

## 3. Register once

Open a pull request against the index repository that only adds your entry to `registry.json`:

```json
{ "id": "author@name", "repo": "owner/repo", "tags": ["resolver", "video"] }
```

- `id` must equal the manifest `identity`.
- One repository publishes one plugin.
- `tags` are optional (up to 10, lowercase) and shown in the market.

A bot downloads your latest release, runs the full admission checks and comments with the result, the permissions you declare and the URL patterns your plugin intercepts. Fix the release and push to the pull request to re-run it. A maintainer reviews and merges, and the plugin appears in the market within minutes.

## 4. Ship updates

Publish a new release with a higher version — no pull request needed. The index checks for new releases every hour. If a release cannot be ingested (wrong identity, version not bumped, malformed manifest…), an issue labeled `stage-fail` is opened in the index repository and closed automatically once a fixed release goes through.

The full admission rules — size limits, zip layout and manifest checks — are in [Packaging & Plugin Market](/docs/en/plugins/packaging/#publishing-to-the-plugin-index) and in the index repository's [README](https://github.com/zerx-lab/fluxdown-plugin-index#publishing-a-plugin).

## Improving the SDK and the plugin docs

The SDK source is [`packages/plugin-sdk`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk) in the main repository, next to these docs. Open pull requests against `main` — see [Contributing to the Docs](/docs/en/contributing/docs/).

## Reporting a problem with a published plugin

Bugs in a plugin belong in that plugin's own repository, linked from its entry in the market. If a published version is broken or harmful, open an issue in the [index repository](https://github.com/zerx-lab/fluxdown-plugin-index/issues): maintainers can yank it, and versions marked `vulnerable` or `malicious` are also disabled where they are already installed.
