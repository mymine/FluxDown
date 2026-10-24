---
title: Contributing Sniffing Rules
description: Share a sniffing rule with every FluxDown user — maintain your own rule repository (register once, publish a Release and it is picked up automatically) or submit a one-off rule; plus how to write rule tests.
section: contributing
order: 3
---

The official rules and an index of community rule repositories live in the public repository [zerx-lab/fluxdown-sniff-rules](https://github.com/zerx-lab/fluxdown-sniff-rules) (the "rules repo" below). A rule that fixes a site for you will probably fix it for everyone, and you don't need to wait for an extension release: once a rule is picked up it is signed automatically and delivered through the desktop app. What each rule field means is covered in the [Sniffing Rules Reference](/docs/en/browser-extension/sniff-rules/).

## Two ways to contribute

| | Maintain your own rule repository (recommended for long-term maintainers) | One-off submission (Issue form) |
| --- | --- | --- |
| Where the rules live | Your own public repository | `rules/community/` in the rules repo |
| Rule id | `community.<your-namespace>.<name>` | `community.misc.<name>` |
| First time | Open a registration PR that only touches `registry.json`; a maintainer reviews it | Open an Issue; a bot opens a PR; a maintainer reviews it |
| Later updates | **Just publish a Release — picked up automatically within an hour, no manual review** | A new Issue and a new review every time |
| Sites you can cover | Only the sites you claimed at registration (exclusive) | Any site that is **not claimed** |

Which one to pick:

- You only want to add one rule for a site and won't maintain it → **one-off submission**, the least work.
- You want to maintain rules for one or more sites over time → **maintain your own repository**. It is reviewed once at registration; after that you control the update pace.
- The site you care about is already claimed by someone else's repository → send a Pull Request to their repository (see [Receiving contributions](#receiving-contributions-from-others)); you can't claim it again.

## Maintain your own rule repository

You only need a GitHub account. The order is: create the repository → write rules → get them working locally → self-check → publish a Release → register (once) → from then on, only publish Releases.

### 1. Create the repository from the template

1. Open [zerx-lab/fluxdown-sniff-rules-template](https://github.com/zerx-lab/fluxdown-sniff-rules-template).
2. Click **Use this template → Create a new repository**.
3. The repository **must be Public** (the rules repo can't read Releases of a private repository).

The template ships a sample `sniff-rules.json` and two workflows: `check` (self-check) and `release` (publishing).

### 2. Edit `sniff-rules.json`

The file sits at the repository root:

```json
{
  "namespace": "alice",
  "hosts": ["example.com", "example-cdn.net"],
  "rules": [
    {
      "id": "community.alice.example-video",
      "version": 1,
      "tier": "community",
      "title": "Example: recognise .xyz files as video",
      "stage": "response",
      "match": { "host": ["example.com"], "ext": ["xyz"] },
      "action": { "kind": "resource", "type": "video" }
    }
  ],
  "tests": [
    {
      "name": "xyz is video",
      "call": "classifyResponse",
      "args": [{ "url": "https://cdn.example.com/a.xyz", "pageUrl": "https://example.com/", "size": 0, "attachment": false }],
      "expect": { "decision": "resource", "type": "video" }
    }
  ]
}
```

| Field | Requirement |
| --- | --- |
| `namespace` | Required. 2–31 lowercase letters / digits / hyphens, starting with a letter or digit (for example your GitHub username). It **must not** be a reserved word: `misc`, `official`, `builtin`, `fluxdown`, `community`, `local`, `plugin`, `test`. Unique across the rules repo; can't be changed after registration. |
| `hosts` | The sites you intend to claim. Only used for the self-check before registration; once registered, the claim in the rules repo's `registry.json` is what counts. |
| `rules[].id` | Must be `community.<namespace>.<name>`, unique across the rules repo. |
| `rules[].tier` | Must be `community`. |
| `rules[].version` | A positive integer; bump it when you change a published rule (see [Updating](#updating)). |
| Site scope | Every `host` / `pageHost` in a rule (including inside `exclude`) and every `mirror.hosts` entry must fall inside your claimed sites (including all their subdomains). |
| Produced URLs | Download URLs produced by `extract` / `dom` rules are only accepted by the extension when their domain falls inside the rule's own `host` / `pageHost`; others are dropped. |
| `tests` | Optional but strongly recommended; see [Writing rule tests](#writing-rule-tests). |

The other rule fields and the community-tier restrictions (must have `host` or `pageHost`, no `preempt`, no global `strip-params`, …) are in the [Sniffing Rules Reference](/docs/en/browser-extension/sniff-rules/#limits-by-tier).

### 3. Get it working in the extension first, then change the id

The easiest way: write the rule under **Settings → Sniffing rules → Local rules** and verify it with the rule tester, paste the JSON into `sniff-rules.json`, then:

1. change `id` from `local.xxx` to `community.<your-namespace>.xxx`;
2. change `tier` from `local` to `community`;
3. add `host` / `pageHost` (required for community rules).

### 4. Self-check

After you push, the template's `check` workflow runs on every push and performs exactly the checks used when your rules are picked up (it calls the rules repo's reusable `author-check` workflow). You can also run it locally; it needs Node.js 22 or newer and no dependencies:

```bash
git clone https://github.com/zerx-lab/fluxdown-sniff-rules
node fluxdown-sniff-rules/scripts/check-source.mjs --file sniff-rules.json --repo <your-username>/<repo-name>
```

On success it prints `✅ 通过：N 条规则、M 条测试` (the script's output is in Chinese); otherwise it lists each problem. Before registration it checks against the `hosts` in your file and against existing claims for conflicts.

### 5. Publish a Release

The rules repo only reads the asset named `sniff-rules.json` in your repository's **Latest Release**.

- **Recommended: push a `v*` tag.** The template's `release` workflow self-checks, then creates the Release and uploads `sniff-rules.json`:

  ```bash
  git tag v1.0.0
  git push origin v1.0.0
  ```

- **Manual** also works: create a Release on GitHub; the asset must be named `sniff-rules.json`, and the Release **must not be a draft or a pre-release**.

### 6. Register (once)

1. Fork [zerx-lab/fluxdown-sniff-rules](https://github.com/zerx-lab/fluxdown-sniff-rules).
2. In your fork, edit `registry.json` and append your entry to the end of `sources`:

   ```json
   { "ns": "alice", "repo": "alice/fluxdown-rules", "hosts": ["example.com", "example-cdn.net"], "description": "Video detection for Example" }
   ```

3. Open a Pull Request. **It may only change `registry.json`** — touching any other file makes the bot fail the check.

| Field | Requirement |
| --- | --- |
| `ns` | Same as `namespace` in `sniff-rules.json`; same format and reserved words. |
| `repo` | `owner/name`; one repository can register only one namespace. |
| `hosts` | The sites you claim: plain domains that automatically include all subdomains; no `=`, `*`, scheme or path; at least two labels (`com` is rejected); at most 32; must not overlap anyone else's claims. |
| `description` | Optional, at most 200 characters. |

Then:

1. The `registry-check` bot fetches your repository's Latest Release, runs the same checks as the pick-up, and comments on the PR: when it passes it lists the rules (id, version, stage, title); when it fails it lists what to fix. Fix it, push a new commit to the PR (or publish a new Release in your repository and re-run the check) to be checked again.
2. If the PR author isn't the owner of `repo`, the bot asks maintainers to confirm the owner agrees; if you are **changing the repository of an existing namespace**, the original maintainer has to confirm in the PR.
3. A maintainer reviews **whether the claimed scope is reasonable** (see [Review and safety boundaries](#review-and-safety-boundaries)) and merges.
4. After the merge, the rules repo picks up your Release **within an hour**.

### Updating

After registration, updates need no PR and no manual review:

1. Edit `sniff-rules.json`. **Bump `version` of every rule you changed.**
2. Publish a new Release by pushing a new tag (for example `v1.0.1`).
3. The rules repo picks it up every hour (at minute 17), then signs and publishes.
4. Time to take effect: the rules repo picks up and signs **within 1 hour**, the desktop app checks the rules repo every 6 hours, and the extension syncs from the desktop app every 6 hours. So from publishing a Release to it working in a user's extension takes **at most roughly a dozen hours**. Pressing **Sync now** on the extension's **Sniffing rules** page pulls whatever the desktop app already has right away.

Users only receive community rules while **Enable community rules** is on and the desktop app is running.

### When pick-up fails, and what is rejected

When a pick-up fails, the rules repo opens an issue titled `stage-fail: <your namespace>` and mentions the repository owner; **the previously picked-up version keeps working**. After you fix it and publish a new Release, the issue is closed automatically on the next successful pick-up.

These are rejected:

- **Replacing the asset of an already picked-up Release** (re-uploading the same asset name) — treated as tampering. Always publish a new Release; never edit an old one.
- **The latest Release is not newer** than the one already picked up.
- **Rule content changed but `version` did not increase.**
- **Sites outside your claim**: a rule's `host` / `pageHost` / `mirror.hosts` falls outside the registered claim.
- **Validation fails**, or **a rule test fails** (tests run together with all rules in the rules repo, see below).
- The Release has no asset named `sniff-rules.json`, or it is a draft / pre-release.

### Widening the claim, switching repositories, stopping

All of these are PRs to the rules repo that edit your own entry in `registry.json`:

- **Widen the claim**: add sites to `hosts`. Maintainers review the added sites, and they must not overlap anyone else's claims.
- **Switch repository**: change `repo`. The original maintainer must confirm in the PR.
- **Stop maintaining**: delete your entry. After the merge the pick-up job removes it and your rules disappear from the community pack.

### Receiving contributions from others

Your rule repository is an ordinary GitHub repository: people who want to improve your rules open a Pull Request against **your repository**. Review and merge them, then publish a new Release as in [Updating](#updating) (remember to bump `version`). The rules repo takes no part in this and never merges anything for you.

## One-off submission

Use this when you don't want a repository. The rule is stored as `community.misc.<name>` in the rules repo's `rules/community/<site>.json`.

1. **Write a local rule and verify it.** Settings → **Sniffing rules** → **Local rules** → **New**; use the rule tester in the same panel, check the verdict, and also test a URL that should **not** match.
2. **Click "Submit to community" on the rule row.** The extension validates it against the community requirements and rewrites `local.x` to `community.misc.x`; if it passes, it opens a pre-filled GitHub Issue. If the rule is too long for the link, the extension copies it to the clipboard and opens an empty form — paste it into the "Rule JSON" field.
3. **Fill in the Issue** (table below) and submit.
4. **A bot validates it**: rule validation, rule tests, and whether the site is already claimed by a registered repository.
   - Failed: the bot replies with the reason; **edit the Issue** to re-validate automatically.
   - Passed: the bot opens a Pull Request and replies with the link.
5. **A maintainer reviews the Pull Request** (see [Review and safety boundaries](#review-and-safety-boundaries)).
6. **After the merge it is signed and published automatically**; the same 6 h desktop + 6 h extension sync intervals apply.

**Sites already claimed by a registered repository are not accepted here**; the bot tells you to contribute to that repository instead.

### The Issue form

| Field | Required | What to enter |
| --- | --- | --- |
| Rule JSON | yes | Pre-filled by the button. You can also paste a single rule, an array, or an exported rules file / `{ "rules": [...], "tests": [...] }`. Rules named `local.xxx` are renamed to `community.misc.xxx`; other tiers are rejected. |
| Example page | yes | The full `http(s)` address of a real page where the rule works. If its domain isn't covered by any rule's `host` / `pageHost`, the bot warns you. |
| Example resource URLs | no | Resource URLs the rule should match (or ignore), one per line, for reviewers. |
| Test cases | no | A JSON array of tests (format below); they are added to the rule file. Rules with tests are reviewed faster. |
| Notes | no | What the rule fixes and why it is written this way. |
| Confirm | yes | Tick to confirm you verified the rule as a local rule and kept the site scope minimal. |

The bot writes the rule to `rules/community/<site>.json`, named after the first `host` / `pageHost` of the first rule (without `=` and `www.`). If the file already exists: rules with the same `id` are replaced (when the content changes, `version` becomes at least "old version + 1"), new rules are appended, and tests with the same `name` are replaced. Every later change needs a new Issue and a new review.

## Review and safety boundaries

**Manual review at registration.** Maintainers review whether the scope of sites you claim is reasonable; the bot's comment lists all rules currently in your repository for reference.

**After that, pick-up is automatic and safety rests on these constraints:**

- **Namespace**: your rule ids must start with `community.<your-namespace>.`; you can't impersonate others or use reserved words.
- **Exclusive claims**: every `host` / `pageHost` / `mirror.hosts` in your rules must fall within your registered claim; claims may not overlap; one-off submissions (`misc`) may not touch claimed sites either.
- **Produced-URL restriction**: URLs produced by a community rule's `extract` / `dom` must be on a domain inside that rule's own `host` / `pageHost`, otherwise the extension drops them — a rule can't send users to sites it never declared. Community rules also can't use `preempt` or global `strip-params`.
- **Immutable versions**: replacing an already picked-up Release asset is rejected; changing rule content without raising `version` is rejected; a latest Release that isn't newer than the picked-up one is rejected.
- **Uniform validation and tests**: before pick-up, all rules are checked with the same validator as the extension and all rule tests are run; any failure blocks the pick-up.
- **Uniform signing**: authors hold no keys; the rules repo signs and publishes, and the desktop app verifies the signature before delivering anything.
- **Revocation and removal**: a problematic rule can be revoked through `revocations.json` (any tier); deleting an entry from `registry.json` removes the whole namespace.

## Writing rule tests

Each test calls one function of the rule engine and compares the result with `expect`. The engine is compiled from **the built-in rules + the official rules + all community rules (every registered repository's rules and the `misc` rules) + the revocation list**. **Your tests run together with all rules in the rules repo**: a pick-up only goes through if everything passes, so avoid tests that other rules can influence, and make sure your rules don't make anyone else's tests fail.

| Field | Meaning |
| --- | --- |
| `name` | Required, unique per rule file recommended; the bot replaces tests with the same name. |
| `call` | One of the functions below. |
| `args` | Array of the function's arguments, in order. |
| `expect` | The expected result. |
| `options` | Optional: `{ "sniffImages": true }` switches image sniffing on for this test (default off). |

**Comparison rules.** For an object in `expect`, only the keys you write are compared (extra keys in the result are fine); an array must have the **same length** and is compared item by item; anything else (strings, numbers, `null`, booleans) must be exactly equal. Because the engine also contains the built-in and official rules, results that are lists (`domRules`, `bodyWatchers`, `extractFromBody`, `mirrorGroups`, …) may contain entries from other rules — pick URLs that only your rule touches, and write the list in full.

| `call` | `args` | Result |
| --- | --- | --- |
| `classifyResponse` | `[{ url, pageUrl, size, attachment, mime?, status?, resourceType?, headers? }]` (`size` and `attachment` are required; `size` `-1` or `0` = unknown; `headers` is an object) | `{ decision: "resource", type, confidence?, rule: { id, version, tier } }`, `{ decision: "ignore", rule }` or `{ decision: "none" }` |
| `classify` | `[url, mime?]` | A resource type such as `"video"` |
| `isIgnored` | `[url, pageUrl, attachment, mime?]` (pass `mime` to apply MIME-based `exclude` the same way `classifyResponse` does) | The matching rule reference `{ id, version, tier }`, or `null` |
| `dedupeKey` | `[url, pageUrl]` | The normalised URL string used to detect duplicates |
| `fileExtension` | `[url, pageUrl]` | The extension (`"zip"`), or `""` |
| `trackKind` | `[{ url, pageUrl, filename?, mime? }]` | `"video"`, `"audio"` or `null` |
| `bodyWatchers` | `[pageUrl]` | A list of `{ rule, url?, host?, pathRegex? }` |
| `extractFromBody` | `[apiUrl, pageUrl, json]` | A list of `{ rule, emits: [{ mode, url, filename?, type?, quality? }] }` |
| `manifestsFromBody` | `[apiUrl, pageUrl, json]` | A list of the JSON sub-trees treated as DASH manifests |
| `domRules` | `[pageUrl]` | A list of `{ rule, selector, attr, emit }` |
| `pageHints` | `[pageUrl]` | `{ titles: [{ selector, attr? }], players: [{ selector }] }` |
| `requestHeaders` | `[url, pageUrl]` | An object of header name (lowercase) → value |
| `mirrorGroups` | `[]` | A list of host lists |

Example 1 — a `response` rule and a negative test (the second one makes sure the rule doesn't leak to other sites):

```json
{
  "tests": [
    {
      "name": "xyz on example.com is video",
      "call": "classifyResponse",
      "args": [{ "url": "https://cdn.example.com/a.xyz", "pageUrl": "https://example.com/", "size": 0, "attachment": false }],
      "expect": { "decision": "resource", "type": "video", "rule": { "id": "community.example.video" } }
    },
    {
      "name": "other sites are not affected",
      "call": "classifyResponse",
      "args": [{ "url": "https://cdn.other.com/a.xyz", "pageUrl": "https://other.com/", "size": 0, "attachment": false }],
      "expect": { "decision": "none" }
    }
  ]
}
```

Example 2 — an `extract` rule (the API response is the third argument):

```json
{
  "tests": [
    {
      "name": "direct link is extracted",
      "call": "extractFromBody",
      "args": [
        "https://api.example.com/play?id=1",
        "https://example.com/watch/1",
        { "code": 0, "data": { "url": "https://cdn.example.com/v/1.mp4", "title": "Episode 1" } }
      ],
      "expect": [
        {
          "rule": { "id": "community.example.play-api" },
          "emits": [{ "mode": "resource", "url": "https://cdn.example.com/v/1.mp4", "filename": "Episode 1" }]
        }
      ]
    },
    {
      "name": "failed response yields nothing",
      "call": "extractFromBody",
      "args": ["https://api.example.com/play?id=1", "https://example.com/watch/1", { "code": 1 }],
      "expect": []
    }
  ]
}
```

## Revoking rules and official rules

- **Revoking a rule**: a wrong or harmful rule is withdrawn through `revocations.json` at the rules repo root rather than silently deleted. Each entry names the `ruleId`, which `versions` it affects (`"*"` or `{ "max": N }`), whether it is a `soft` or `hard` revocation, and a `reason`; what users see is described under [Revocation](/docs/en/browser-extension/sniff-rules/#revocation). Describe the problem in an Issue or Pull Request on the rules repo and a maintainer handles it. For a rule in your own repository, the simpler fix is to correct it and publish a new Release (with `version` bumped).
- **Official rules** (`rules/official/`) are maintained by the maintainers and may use privileges such as `preempt` and global `strip-params`.

## How publishing works

The rules repo's `publish` workflow runs **every hour** (and also when rule sources change on `main`): it first picks up new Releases of registered repositories, then validates all rules, runs the tests, and finally signs the official pack, the community pack and the revocation list. Every signed document carries a monotonically increasing `sequence`; the desktop app verifies the signature and ignores anything older than what it already has. The community pack and the revocation list are valid for 30 days and are re-signed automatically when fewer than 14 days remain, so they never expire even without changes. The signing key lives only in a Secret of the rules repo.
