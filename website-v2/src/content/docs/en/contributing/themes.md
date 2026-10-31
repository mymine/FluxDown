---
title: Submitting Themes
description: Design a theme, package it with screenshots and submit it to the FluxDown theme market through a pull request.
section: contributing
order: 2
---

The theme market is the public repository [zerx-lab/fluxdown-themes](https://github.com/zerx-lab/fluxdown-themes). Each theme is one directory; when a pull request is merged, CI regenerates the repository's `index.json` and the theme appears in the [theme market](/themes/) and in the app. The repository's [CONTRIBUTING.md](https://github.com/zerx-lab/fluxdown-themes/blob/main/CONTRIBUTING.md) is the authoritative spec — this page walks you through it.

## 1. Design the theme

Open the [Theme Builder](/theme-builder/), pick a dark or light base preset and edit the tokens against the live preview. With the **Flutter** client selected, **Export** writes the theme format the market's variant files use (`schemaVersion: 2`). You can also tune a theme in the app under **Settings → Appearance** and export its JSON there.

Make one variant (dark or light) or both. A theme can also carry a version for the GPUI client — the market shows it with a **GPUI** badge, and the builder's **GPUI** mode exports that format (`fluxdown.gpui-theme` v2). Check the repository's CONTRIBUTING.md for how to include it.

## 2. Lay out the directory

Each theme lives in `themes/<theme-id>/`. **A pull request may change only one theme directory** (enforced by CI).

| File | Required | Notes |
| --- | --- | --- |
| `manifest.json` | Yes | Market metadata, see below |
| `theme.dark.json` / `theme.light.json` | At least one | The exported theme files |
| `screenshot-dark.png` / `screenshot-light.png` | One per variant | See [Screenshots](#3-take-screenshots) |
| `README.md` | No | An introduction to your theme |

`theme-id` rules:

- kebab-case — lowercase letters, digits and hyphens (`^[a-z0-9]+(-[a-z0-9]+)*$`), 3–40 characters, unique in the repository.
- Reserved for built-in themes: `default`, `default-dark`, `default-light`, `midnight-blue`, `nord`, `warm-light`, `graphite`, `graphite-light`.

`manifest.json`:

```json
{
  "manifestVersion": 1,
  "id": "ocean-breeze",
  "name": "Ocean Breeze",
  "author": "your-github-username",
  "version": "1.0.0",
  "description": "A calm deep-sea palette.",
  "tags": ["dark", "blue", "minimal"],
  "variants": {
    "dark": "theme.dark.json",
    "light": "theme.light.json"
  }
}
```

- `id` must match the directory name.
- `version` is semver (`x.y.z`); bump it whenever you update the theme.
- `author` should be your GitHub username — the market links to your profile.
- `tags` are optional: 0–8 tags, each at most 20 characters.
- `variants` needs at least one key (`dark` / `light`), each pointing to a theme file in the directory.

In each theme file, `appearance` must match its variant key (`"appearance": "dark"` in `theme.dark.json`). Colors are hex strings (`aarrggbb` or `rrggbb`, `#` optional). Missing colors fall back to the defaults for that brightness, but filling in every `colors` group is recommended. `metrics` is optional, and `segmentPalette` holds at most 32 colors. [`themes/ocean-breeze/`](https://github.com/zerx-lab/fluxdown-themes/tree/main/themes/ocean-breeze) is a minimal example.

## 3. Take screenshots

- **PNG**, named `screenshot-dark.png` / `screenshot-light.png`.
- Landscape window ratio between 1.5 and 1.85 (16:10 or 16:9, e.g. 1280×800 or 1920×1080), 1200–4000 px wide.
- At most **500 KB** each — [Squoosh](https://squoosh.app) compresses well.
- Show the FluxDown main window with a few download tasks.
- **No personal information** — real file names, URLs or user names. Downloading a few Linux ISOs makes good demo tasks.

## 4. Open a pull request

1. Fork the repository and add `themes/<theme-id>/` with your files.
2. Optionally validate locally: `node scripts/validate.mjs`.
3. Open a pull request and fill in the template.
4. CI checks the manifest, id uniqueness, theme file format, screenshot size and dimensions, and that only one theme directory changed.
5. A maintainer looks over the screenshots and merges. CI updates `index.json` and the theme is live.

## Updating a theme

Edit your theme directory and bump `version` in `manifest.json`. Changes to someone else's theme need the original author's confirmation or a maintainer's decision.

Submitted themes are distributed under the repository's [license](https://github.com/zerx-lab/fluxdown-themes/blob/main/LICENSE).
