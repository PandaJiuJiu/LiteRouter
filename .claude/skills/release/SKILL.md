---
name: release
description: Release a new LiteRouter version end to end — bump the version, write the CHANGELOG entry, commit, tag, push, and publish the GitHub Release. Use this whenever the user asks to release, cut a version, publish a version, ship a release, or mentions "发版" / "发布新版本" / "打个 tag" / "release 一下" — even if they only say "release 一下" without naming a version. It also covers preparing a release without pushing (dry run) and fixing a release that went out wrong.
---

# Release LiteRouter

A release here is **one version number in three places** plus a tag and a GitHub
Release. The whole point of this skill is that those never drift apart —
they already did once, and the result was a repo where `v0.0.18` shipped
product `0.0.16`.

## The three places a version number lives

| Place | File | What it drives |
|---|---|---|
| Backend | `backend/Cargo.toml` | `CARGO_PKG_VERSION` → the version shown on the Settings page, and the Docker image's binary |
| Frontend | `frontend/package.json` | The frontend's own version string |
| Lockfiles | `backend/Cargo.lock`, `frontend/package-lock.json` | Generated — never hand-edit, let the toolchain update them |

Plus two release artifacts:

| Artifact | Where it lands | How it gets there |
|---|---|---|
| Docker image | `ghcr.io/pandajiujiu/literouter:<tag>` | CI, triggered by pushing a `v*` tag |
| GitHub Release | `github.com/PandaJiuJiu/LiteRouter/releases` | `gh release create`, run by hand |

**The tag and the product version must be the same string.** `v0.0.19` ships
`0.0.19`. If a past release left them apart, the new release is where the
divergence ends — don't carry it forward.

## Steps

Work through these in order. Each step says how to know it's done.

### 0. Before anything: check the tree and pick the version

```bash
git status -sb                    # must be clean
git log --oneline v0.0.18..HEAD   # what's shipping
git tag --sort=-creatordate | head -3
```

The version is the **next** patch bump of the highest existing tag, and it
equals the next bump of the product version in `backend/Cargo.toml` — take
whichever is higher, and make them the same. E.g. tags at `v0.0.18` and
product at `0.0.16` → ship `0.0.19`.

Confirm the number with the user before writing anything. It goes in three
files and a tag; renaming it later means re-tagging and re-pushing an image.

### 1. Run the full gate

Everything CI runs, run locally first — a red tag push is a failed release.

```bash
cd backend && cargo fmt --check --all && cargo clippy --all-targets -- -D warnings && cargo test --all-targets
cd ../frontend && npm test && npm run build
```

`npm test` is `check:i18n` + vitest. Both must pass. Do not skip `npm run
build` — the Docker image builds the frontend, and a build error only
surfaces there.

**If something fails: fix it first, then release.** A release that carries a
red test is worse than no release.

### 2. Bump the version

Edit `backend/Cargo.toml` and `frontend/package.json`. Then let the
toolchains update their lockfiles:

```bash
cd backend && cargo update -p literouter --offline 2>/dev/null || cargo check --offline
cd ../frontend && npm install --package-lock-only --ignore-scripts
```

`npm install --package-lock-only` rewrites `package-lock.json` to match
`package.json` without touching `node_modules` or running scripts. Verify
all four files now agree:

```bash
grep -m1 '^version' backend/Cargo.toml
grep -n -A2 'name = "literouter"' backend/Cargo.lock | grep version
grep -m1 '"version"' frontend/package.json
grep -m1 '"version"' frontend/package-lock.json
```

All four must read the same number.

### 3. Write the CHANGELOG entry

Add a section at the top of `CHANGELOG.md`, above the current newest entry:

```markdown
## [0.0.19] - 2026-10-22

自 v0.0.18 起共 10 个提交，<一句话概括这批改动的共同主题>。

### 新增

- **<用户能感知的改动>**：<为什么改，而不是怎么改>

### 修复

- **<用户能感知的改动>**：<症状 → 根因>

### 其他

- <流程 / 文档 / 依赖层面的改动>
```

Rules for the content:

- **Write it for someone running the gateway, not for a reader of the diff.**
  Say what changed for them, not which function was edited.
- **Lead with the symptom.** "OpenAI-shaped upstreams recorded zero cache
  hits — 12,718 requests against upstreams reporting 250,000 cached tokens a
  call" tells the reader whether they are affected. "Refactored the usage
  parser" does not.
- **The commits are the source, not the summary.** Read them:
  `git log v0.0.18..HEAD --format='%H %s%n%n%b'`. The bodies of this repo's
  commits are detailed and already contain the reasoning — use them, but
  translate into the user-facing register above.
- **Group by 新增 / 修复 / 其他**, matching the existing entries. If a
  category is empty, drop it.
- **Mention database migrations by name.** They run automatically on
  startup, but a migration that rewrites historical rows is worth naming.
- **Keep the `自 vX 起共 N 个提交` line** — it's the established convention
  in this file.

Then add the link at the bottom of the file, in the existing block:

```markdown
[0.0.19]: https://github.com/PandaJiuJiu/LiteRouter/releases/tag/v0.0.19
```

Newest first, matching the existing order.

### 4. Commit

```bash
git add -A
git commit -m "release: v0.0.19 — <short theme>"

# body, if there is more to say:
git commit -m "release: v0.0.19 — <short theme>" -m "<what changed, why>"
```

The subject convention is `release: vX.Y.Z — <theme>`. The theme is a
short noun phrase, in the same register as the existing entries (`release:
v0.0.16 — GHCR automation, public image, version sync`).

Check the diff before committing — this commit is what the tag will point at:

```bash
git show --stat HEAD
```

### 5. Tag and push

```bash
git tag v0.0.19
git push origin main
git push origin v0.0.19
```

The tag push is what triggers CI's `release` job, which builds the image and
pushes it to GHCR as both `v0.0.19` and `latest`.

**The tag must be pushed to `main` first.** CI checks out the tag's commit;
if the commit itself isn't on `main` yet, the tag build diverges from the
branch.

**Only push the tag once the release commit is on `origin/main`** — a tag on
a commit that later gets amended leaves a tag pointing at a commit that no
longer exists on the branch.

### 6. Watch CI, then create the GitHub Release

```bash
gh run list --limit 5
gh run view <run-id> --log-failed
```

The `release` job only runs after `backend`, `frontend`, and `docker` all
pass. If the `docker` job fails, check the registry — the Dockerfile's
default `REGISTRY=docker.m.daocloud.io/library` is a CN mirror that
intermittently times out from GitHub's US runners. The workflow already
overrides it with `REGISTRY=docker.io/library`; if a run still fails on a
TLS handshake timeout, re-run the job.

**Only create the GitHub Release after CI succeeds** — a Release page
pointing at an image that was never pushed is worse than no page.

The Release body is the CHANGELOG entry plus the pull command:

```bash
gh release create v0.0.19 \
  --title "v0.0.19" \
  --notes-file /tmp/release-notes.md
```

Build the notes file from the CHANGELOG section — don't retype it, and
don't let `--generate-notes` overwrite it with a list of commit subjects.
The Release body should end with:

```markdown
```bash
docker pull ghcr.io/pandajiujiu/literouter:v0.0.19
```
```

A GitHub Release is **not** a second distribution channel. The product ships
as a Docker image; the Packages page lists the image tags, and the Release
page carries the changelog and the pull command. Don't attach binaries —
see "What this release does not do" below.

### 7. Verify

```bash
gh release list --limit 5
gh release view v0.0.19
gh api repos/PandaJiuJiu/LiteRouter/releases --jq '.[].tag_name'
```

And confirm the image landed:

```bash
TOKEN=$(curl -s "https://ghcr.io/token?scope=repository:pandajiujiu/literouter:pull&service=ghcr.io" \
  | sed -E 's/.*"token":"([^"]+)".*/\1/')
curl -s -H "Authorization: Bearer $TOKEN" \
  https://ghcr.io/v2/pandajiujiu/literouter/tags/list
```

`v0.0.19` and `latest` should both be in the tag list.

Finally, pull the image and check it runs — the tag list proves it was
pushed, not that it works:

```bash
docker run --rm -d --name literouter-smoke \
  -e PORT=3000 -e LITEROUTER_DB=/app/data/literouter.db \
  -p 3000:3000 ghcr.io/pandajiujiu/literouter:v0.0.19
sleep 5 && curl -f http://localhost:3000/api/setup-status
docker stop literouter-smoke
```

## What this release does not do

- **No binaries as release assets.** The product ships as a Docker image.
  The backend is a single Rust binary and the frontend is a `dist/` beside
  it, so binaries *would* work — but that is a second distribution channel
  with its own versioning, and it would need cross-compilation in CI. Not
  part of this skill.
- **No backfilling of historical Releases.** Past versions may have tags
  without Release pages. That's history; don't retro-fit it unless asked.
- **No changes to the migration files.** Migrations are immutable. A new
  schema change is a new `00NN_*.sql`.
- **No `--generate-notes`.** The notes come from the CHANGELOG.

## Repo-specific gotchas

- **The image is linux/amd64 only.** The Dockerfile builds no arm64 variant.
  Say so when a release is announced to users on Apple Silicon.
- **Sessions are in-memory.** Every restart logs everyone out. This is
  intentional and is already documented in the README's upgrade notes; don't
  re-litigate it in the release notes.
- **`LITEROUTER_DB` must be set explicitly** when running the prebuilt
  image, or the database lands in the container's writable layer and is
  lost on recreate. Already warned about in the README.
- **The `docker` CI job and the `release` job both build the image.** They
  don't share a cache between them, so a release takes ~10-15 minutes of CI.
  Don't push a tag and then immediately assume the image exists — check.
- **The `gh` CLI on this box is 2.4.0** — old. `gh release list --json` does
  not work; use `gh api` for structured output. `gh release create` with
  `--notes-file` does work.
- **The repo has no CI on `main` being a gate** — CI runs on push and PR,
  but nothing blocks a merge. The local gate in step 1 is the real one.

## The Release-page permission trap

`gh release create` needs a token with **`contents: write`** on the repo. A
fine-grained PAT scoped to the repo can read everything and still fail with:

```
HTTP 403: Resource not accessible by personal access token
```

The confusing part is that the same token pushes tags and branches fine —
git push goes over SSH, which uses the SSH key, not the PAT. So the token's
push permission is a red herring; the Release API is a separate grant.

Check before assuming:

```bash
gh api -X GET user -i 2>&1 | grep -i "x-oauth-scopes"
```

A classic PAT shows `repo` (or `public_repo`) in that header. A fine-grained
PAT shows **nothing** — no `X-OAuth-Scopes` header at all — and that absence
is the tell that the grants are per-repository rather than per-scope.

**If the Release can't be created, the release is not finished.** The image
is already published and the tag exists, but the Releases page stays empty
and the CHANGELOG's `/releases/tag/vX` link stays dead. Say so plainly and
ask for a token with `contents: write` — do not report the release as
complete, and do not strip the Release step from the skill to make it look
like a normal path.

The notes file is written to `/tmp/release-notes.md` before the CI wait, so
it is ready the moment the image lands.
