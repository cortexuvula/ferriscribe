# Item #3: latest.json Release Order Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Eliminate the updater race condition by generating `latest.json` in a dedicated job that runs only after all platform assets are uploaded.

**Architecture:** Suppress per-job `latest.json` generation in `tauri-action` (`updaterJson: false`). Add a `manifest` job with `needs: release` that generates a consolidated `latest.json` via a small Node.js script and uploads it to the release.

**Tech Stack:** GitHub Actions, Node.js inline script, `gh` CLI.

**Spec:** `docs/superpowers/specs/2026-07-08-high-priority-improvements-design.md` (Item #3)

---

## Task 1: Suppress per-job latest.json + add manifest job

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Read the current release workflow**

Read `.github/workflows/release.yml` fully. Understand:
- The `release` job's matrix strategy (3 platforms)
- The `tauri-action` step inputs (around lines 95-104)
- The `prune` job that runs `needs: release` (the pattern to mirror)

- [ ] **Step 2: Add `updaterJson: false` to tauri-action**

In the `tauri-apps/tauri-action` step (find the `with:` block), add:

```yaml
          updaterJson: false
```

This prevents each matrix job from generating its own `latest.json`. Place it alongside the other `with:` inputs (tagName, releaseName, etc.).

- [ ] **Step 3: Add the manifest job**

After the `prune` job (which has `needs: release`), add a new `manifest` job:

```yaml
  manifest:
    needs: release
    runs-on: ubuntu-24.04
    permissions:
      contents: write
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Setup Node
        uses: actions/setup-node@v4
        with:
          node-version: 24

      - name: Generate consolidated latest.json
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          TAG="${GITHUB_REF#refs/tags/}"
          VERSION="${TAG#v}"
          echo "Generating latest.json for $TAG (version $VERSION)"

          # Get release creation date
          PUB_DATE=$(gh release view "$TAG" --json createdAt -q .createdAt)

          # Get release notes (first 500 chars to keep JSON manageable)
          NOTES=$(gh release view "$TAG" --json body -q .body | head -c 500)

          # Generate latest.json from release assets
          node << 'SCRIPT'
          const { execSync } = require('child_process');
          const tag = process.env.GITHUB_REF.replace('refs/tags/', '');
          const version = tag.replace(/^v/, '');
          const repo = 'cortexuvula/rustMedicalAssistant';
          const baseUrl = `https://github.com/${repo}/releases/download/${tag}`;

          // List assets
          const assets = JSON.parse(execSync(`gh release view ${tag} --json assets -q '.assets[].name'`).toString().trim().split('\n').length > 0
            ? execSync(`gh release view ${tag} --json assets`).toString()
            : '{"assets":[]}');
          const assetNames = JSON.parse(execSync(`gh release view ${tag} --json assets`).toString()).assets.map(a => a.name);

          // Map platform assets to Tauri platform keys
          const platforms = {};

          // Find each platform's installer + signature
          const platformMap = {
            'linux-x86_64': { asset: '_linux_amd64.AppImage', sig: '_linux_amd64.AppImage.sig' },
            'windows-x86_64': { asset: '_windows_amd64.nsis.zip', sig: '_windows_amd64.nsis.zip.sig' },
            'darwin-aarch64': { asset: '_darwin_aarch64.app.tar.gz', sig: '_darwin_aarch64.app.tar.gz.sig' },
          };

          // Actually, we need to match the actual asset names. Let's be flexible:
          // Find .sig files and derive the platform from them.
          for (const name of assetNames) {
            if (name.endsWith('.sig')) {
              const baseAsset = name.replace(/\.sig$/, '');
              let platform = null;

              if (name.includes('linux') && name.includes('x86_64')) platform = 'linux-x86_64';
              else if (name.includes('windows') && name.includes('x86_64')) platform = 'windows-x86_64';
              else if (name.includes('darwin') && (name.includes('aarch64') || name.includes('arm64'))) platform = 'darwin-aarch64';
              else if (name.includes('darwin') && name.includes('x86_64')) platform = 'darwin-x86_64';
              else if (name.includes('darwin')) platform = 'darwin-aarch64'; // default mac to arm
              else continue;

              if (!assetNames.includes(baseAsset)) continue;

              const signature = execSync(`gh release download ${tag} --pattern "${name}" --output -`).toString().trim();
              platforms[platform] = {
                signature,
                url: `${baseUrl}/${baseAsset}`,
              };
            }
          }

          const pubDate = execSync(`gh release view ${tag} --json createdAt -q .createdAt`).toString().trim();
          const notes = execSync(`gh release view ${tag} --json body -q .body`).toString().trim().substring(0, 500);

          const manifest = {
            version,
            notes,
            pub_date: pubDate,
            platforms,
          };

          console.log('Generated manifest:', JSON.stringify(manifest, null, 2));
          require('fs').writeFileSync('latest.json', JSON.stringify(manifest, null, 2));
          SCRIPT

      - name: Upload latest.json
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          TAG="${GITHUB_REF#refs/tags/}"
          gh release upload "$TAG" latest.json --clobber
          echo "latest.json uploaded to release $TAG"
```

**IMPORTANT:** The inline Node.js script uses environment variables (`GITHUB_REF`) and `gh` CLI. The script finds `.sig` files in the release assets, reads their contents, and maps them to Tauri platform keys. This is more robust than hardcoding filenames since the exact asset names may vary.

Read the actual asset names from the most recent successful release to verify the matching patterns work. Run: `gh release view v0.28.8 --json assets -q '.assets[].name'` to see actual names.

- [ ] **Step 4: Verify the prune job still has correct `needs`**

The `prune` job should depend on both `release` and `manifest` (or just `release` if you want pruning to happen even if the manifest fails). Check the current `needs` and decide. If `prune` has `needs: release`, consider changing to `needs: [release, manifest]` so pruning waits for the manifest too. But this is optional — pruning old releases doesn't affect the current release's manifest.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "fix(release): generate consolidated latest.json after all platform assets

Suppresses per-job latest.json generation (updaterJson: false) and adds
a dedicated manifest job that runs needs:release. The manifest job reads
all .sig files from the release assets, constructs a multi-platform
latest.json, and uploads it last. Eliminates the updater race condition
where clients see a new version before their platform binary is uploaded."
```

---

## Self-Review

### Spec coverage
- ✅ `updaterJson: false` to suppress per-job generation — Step 2
- ✅ Dedicated `manifest` job with `needs: release` — Step 3
- ✅ Consolidated multi-platform JSON — Step 3 (Node.js script)
- ✅ `latest.json` uploaded last — Step 3 (runs after all release jobs)

### Known caveats
1. The asset name matching patterns (linux/windows/darwin) must match the actual filenames generated by tauri-action. Verify with `gh release view v0.28.8 --json assets -q '.assets[].name'` before relying on them.
2. The Node.js inline script is verbose but self-contained — no external dependencies needed.
3. The `--clobber` flag on `gh release upload` overwrites any stale `latest.json` that might have been created by a race.
