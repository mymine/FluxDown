---
title: Ways to Contribute
description: Where each kind of contribution goes — plugins, themes, sniffing rules, translations, docs and code — and which public repository to use.
section: contributing
order: 0
---

FluxDown is developed in the open across a few public GitHub repositories. Each kind of contribution has its own home and its own workflow — find the row that matches what you want to share.

| You want to share | Repository | How it gets in | Guide |
| --- | --- | --- | --- |
| A plugin (resolver, hook, view) | [zerx-lab/fluxdown-plugin-index](https://github.com/zerx-lab/fluxdown-plugin-index) | Register your own plugin repository once; every later GitHub Release is picked up automatically | [Publishing Plugins](/docs/en/contributing/plugins/) |
| A theme | [zerx-lab/fluxdown-themes](https://github.com/zerx-lab/fluxdown-themes) | Pull request with the theme files and screenshots; CI validates, a maintainer reviews | [Submitting Themes](/docs/en/contributing/themes/) |
| A sniffing rule for the browser extension | [zerx-lab/fluxdown-sniff-rules](https://github.com/zerx-lab/fluxdown-sniff-rules) | Your own rule repository (registered once) or a one-off Issue form | [Contributing Sniffing Rules](/docs/en/contributing/sniff-rules/) |
| A translation | [zerx-lab/FluxDown](https://github.com/zerx-lab/FluxDown) | Pull request against `main` | [Translating FluxDown](/docs/en/contributing/translations/) |
| A docs fix or a code change | [zerx-lab/FluxDown](https://github.com/zerx-lab/FluxDown) | Pull request against `main` | [Contributing to the Docs](/docs/en/contributing/docs/) |

## Tools for contributors

- **Plugin SDK** — [`fluxdown-plugin-sdk`](https://www.npmjs.com/package/fluxdown-plugin-sdk) on npm: typed `flux.*` APIs, a JSX runtime for views, the manifest validator used by the engine and the plugin index, and a CLI that packs `.fxplug` files. Its source is [`packages/plugin-sdk`](https://github.com/zerx-lab/FluxDown/tree/main/packages/plugin-sdk) in the main repository; usage is covered in [Plugin SDK & CLI](/docs/en/plugins/sdk/).
- **Theme Builder** — the [Theme Builder](/theme-builder/) edits every theme token against a live preview of the desktop client and exports theme JSON.
- **Rule repository template** — [zerx-lab/fluxdown-sniff-rules-template](https://github.com/zerx-lab/fluxdown-sniff-rules-template) starts your own sniffing-rule repository with the self-check workflow already set up.

## Other ways to help

- Report bugs and request features on the [feedback page](/feedback/) or in [GitHub Issues](https://github.com/zerx-lab/FluxDown/issues).
- Vote on what gets built next on the [feature vote](/feature-vote/) page.
- Every docs page has an **Edit this page** link and a feedback form at the bottom.

## Good to know

- In the main repository, open pull requests against **`main`** — it is the development branch; `stable` only tracks stable releases.
- Contributions are accepted under the license of the repository you contribute to; the main repository is AGPL-3.0.
- Plugin and sniffing-rule authors keep their work in their own repositories. The index repositories only register them, so you stay in control of your release pace.
