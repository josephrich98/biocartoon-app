#!/usr/bin/env bash
# Deploy BioCartoon. Expects the biocartoon and biocartoon-app checkouts side by side.
#
#   ./deploy.sh all "MESSAGE"     brand + db + workers + web + site + desktop + plugin, then push both repos
#   ./deploy.sh brand             copy the logo and its renders from app/assets/ to site/public/ and biocartoon/brand/
#   ./deploy.sh sync [--apply]    new upstream icons -> master DB (dry run without --apply; --apply also runs db)
#   ./deploy.sh db                master DB -> R2 (also merges approved app uploads, rewrites the READMEs, site table and icon counts)
#   ./deploy.sh workers           D1 migrations, then the submit + review + MCP Workers (regenerates MCP metadata)
#   ./deploy.sh plugin            build the ChatGPT + Claude plugin ZIPs; warn what needs a release or an upload
#   ./deploy.sh plugin --uploaded [chatgpt|claude]   mark HEAD as uploaded (moves the portal tag; default both)
#   ./deploy.sh web               app.biocartoon.com
#   ./deploy.sh site              biocartoon.com
#   ./deploy.sh desktop           rebuild and reinstall /Applications/BioCartoon.app
#   ./deploy.sh push "MESSAGE"    git add -A, commit, push in both repos (skips a repo with nothing to commit)
#   ./deploy.sh release X.Y.Z     bump the version, commit, tag, push: PyPI + GitHub Release (5-15 min)
#   ./deploy.sh release X.Y.Z --no-push   the same, but stop before pushing (to check the bump)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAT="$ROOT/biocartoon"
APP="$ROOT/biocartoon-app"
source "$HOME/.cargo/env" 2>/dev/null || true

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# app/assets/ holds the one copy of the brand art that gets edited (logo.svg, the source of every
# app icon via `cargo run --bin make_icon`, and biocartoon-mono.svg). The site and the catalog repo
# keep mirrors because each deploys on its own; this refreshes them.
BRAND_MIRRORS=(
  "app/assets/logo.svg:site/public/logo.svg"
  "app/assets/icon.png:site/public/logo.png"
  "app/assets/apple-touch-icon.png:site/public/apple-touch-icon.png"
  "app/assets/logo.svg:$CAT/brand/logo.svg"
  "app/assets/biocartoon-mono.svg:$CAT/brand/biocartoon-mono.svg"
)

brand() {
  step "brand art"
  local pair src dst n=0
  for pair in "${BRAND_MIRRORS[@]}"; do
    src="$APP/${pair%%:*}" dst="${pair#*:}"
    [[ "$dst" = /* ]] || dst="$APP/$dst"
    if ! cmp -s "$src" "$dst"; then
      cp "$src" "$dst"; echo "updated ${dst#"$ROOT"/}"; n=$((n + 1))
    fi
  done
  [ "$n" -gt 0 ] || echo "mirrors match app/assets/"
}

sync_upstream() {
  step "sync upstream icons ($*)"
  (cd "$CAT" && python3 data/sync.py "$@")
}

db() {
  step "publish catalog to R2"
  (cd "$CAT" && python3 data/publish_catalog.py)
}

mcp_metadata() {
  step "MCP tool metadata"
  # The hosted MCP Worker serves worker/generated/mcp-metadata.json, generated from the Rust server.
  (cd "$CAT/catalog" && cargo build -q --release --locked --bin biocartoon && python3 ../scripts/build_mcp_metadata.py >/dev/null)
  if git -C "$CAT" diff --quiet -- worker/generated/mcp-metadata.json; then
    echo "unchanged"
  else
    echo "updated worker/generated/mcp-metadata.json"
  fi
}

workers() {
  mcp_metadata
  step "D1 migrations + Workers"
  # deploy:mcp is mcp.biocartoon.com (what the ChatGPT/Claude plugins call); it runs the live check after deploying.
  (cd "$CAT/worker" && npm run migrate && npm test && npm run deploy && npm run deploy:mcp)
}

# What each AI-assistant channel freezes. Server-side changes (tools, instructions, gallery) reach
# every channel live on deploy:mcp; these files only reach users through a release or an upload.
#   ChatGPT plugin:  ZIP uploaded to the OpenAI portal                     (tag plugin-portal)
#   Claude plugin:   ZIP uploaded for the Claude apps                       (tag claude-plugin-portal)
#   Claude Code plugin + `biocartoon install-skills`: the next v* release (plugin.json version, PyPI)
CHATGPT_FROZEN=(catalog/skills .codex-plugin/plugin.json mcp.json submission/listing.json submission/review-cases.json submission/assets)
CLAUDE_FROZEN=(catalog/skills .claude-plugin/plugin.json .mcp.json)
RELEASE_FROZEN=(catalog/skills .claude-plugin .mcp.json)

# Files under the given paths that differ (committed or not) from REF.
changed_since() {
  local ref="$1"; shift
  { git -C "$CAT" diff --name-only "$ref" -- "$@"; git -C "$CAT" ls-files --others --exclude-standard -- "$@"; } | sort -u
}

have_tag() {
  git -C "$CAT" rev-parse -q --verify "refs/tags/$1" >/dev/null && return
  git -C "$CAT" fetch -q origin tag "$1" 2>/dev/null || true
  git -C "$CAT" rev-parse -q --verify "refs/tags/$1" >/dev/null
}

plugin() {
  if [ "${1:-}" = --uploaded ]; then
    local which="${2:-both}" tags=()
    case "$which" in
      chatgpt) tags=(plugin-portal) ;;
      claude) tags=(claude-plugin-portal) ;;
      both) tags=(plugin-portal claude-plugin-portal) ;;
      *) die "usage: ./deploy.sh plugin --uploaded [chatgpt|claude|both]" ;;
    esac
    [ -z "$(git -C "$CAT" status --porcelain -- "${CHATGPT_FROZEN[@]}" "${CLAUDE_FROZEN[@]}")" ] \
      || die "plugin files have uncommitted changes; commit what you uploaded first"
    for t in "${tags[@]}"; do
      step "mark $t = $(git -C "$CAT" rev-parse --short HEAD)"
      git -C "$CAT" tag -f "$t"
      git -C "$CAT" push -qf origin "refs/tags/$t"
    done
    return
  fi
  step "assistant plugin packages"
  # The -public-draft ZIP is the ChatGPT portal upload: the plugin plus submission/'s listing, review
  # cases and assets. The plain biocartoon-<version>.zip is the bare plugin.
  (cd "$CAT" && python3 scripts/package_plugin.py && python3 scripts/package_plugin.py --submission \
    && python3 scripts/package_claude_plugin.py)
  echo "live on deploy:mcp: tools, server instructions, gallery (ChatGPT, Claude, Claude Code, Codex)"

  local warn='\033[33mwarning:\033[0m' changed last
  last="$(git -C "$CAT" describe --tags --match 'v[0-9]*' --abbrev=0 2>/dev/null || true)"
  if [ -n "$last" ] && changed="$(changed_since "$last" "${RELEASE_FROZEN[@]}")" && [ -n "$changed" ]; then
    printf "$warn the skills/Claude plugin changed since %s; Claude Code plugin and install-skills users won't get it until a release:\n" "$last"
    echo "$changed" | sed 's/^/    /'
    echo "  Run: ./deploy.sh release X.Y.Z"
  fi

  local label tag paths file which
  for label in ChatGPT Claude; do
    if [ "$label" = ChatGPT ]; then
      tag=plugin-portal; paths=("${CHATGPT_FROZEN[@]}"); file="dist/biocartoon-<version>-public-draft.zip -> the OpenAI portal"; which=chatgpt
    else
      tag=claude-plugin-portal; paths=("${CLAUDE_FROZEN[@]}"); file="dist/biocartoon-claude-<version>.zip -> the Claude plugin upload"; which=claude
    fi
    if ! have_tag "$tag"; then
      printf '\033[33mnote:\033[0m no %s tag, so changes since the last %s upload are unknown. After uploading, run: ./deploy.sh plugin --uploaded %s\n' "$tag" "$label" "$which"
    elif changed="$(changed_since "$tag" "${paths[@]}")" && [ -n "$changed" ]; then
      printf "$warn the %s plugin's bundled files changed since its upload (%s):\n" "$label" "$tag"
      echo "$changed" | sed 's/^/    /'
      echo "  Upload biocartoon/$file, then run: ./deploy.sh plugin --uploaded $which"
    else
      echo "$label plugin: bundled files match the upload; nothing to upload"
    fi
  done
}

web() {
  step "web app"
  # trunk replaces dist/, which also drops the BioCartoon.app that bundle_mac.sh leaves there.
  (cd "$APP/app" && trunk build --release && npx wrangler deploy)
}

site() {
  brand
  step "website"
  # The full-size screenshot (for the README) is scaled to 1920px wide for the site, and the
  # <img> width/height are set to match so the page doesn't shift while it loads.
  local shot="$APP/site/public/app_screenshot.png"
  sips --resampleWidth 1920 "$APP/app/assets/app_screenshot.png" --out "$shot" >/dev/null
  local w h
  w="$(sips -g pixelWidth "$shot" | awk '/pixelWidth/ {print $2}')"
  h="$(sips -g pixelHeight "$shot" | awk '/pixelHeight/ {print $2}')"
  sed -i '' -E "s#(src=\"app_screenshot.png\") width=\"[0-9]+\" height=\"[0-9]+\"#\1 width=\"$w\" height=\"$h\"#" "$APP/site/public/index.html"
  grep -q "src=\"app_screenshot.png\" width=\"$w\" height=\"$h\"" "$APP/site/public/index.html" \
    || die "site/public/index.html: couldn't set the screenshot's width/height"
  (cd "$APP/site" && npx wrangler deploy)
}

desktop() {
  step "local desktop app"
  (cd "$APP/app" && ./bundle_mac.sh --install)
}

push() {
  local msg="${1:?usage: ./deploy.sh push \"MESSAGE\"}"
  for repo in "$CAT" "$APP"; do
    step "push $(basename "$repo")"
    git -C "$repo" add -A
    if git -C "$repo" diff --cached --quiet; then
      echo "nothing to commit"
    else
      git -C "$repo" commit -m "$msg"
    fi
    git -C "$repo" push
  done
}

release() {
  local v="${1:?usage: ./deploy.sh release X.Y.Z [--no-push]}" no_push="${2:-}"
  [[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must look like 1.2.3, got '$v'"

  for repo in "$CAT" "$APP"; do
    local name; name="$(basename "$repo")"
    [ -z "$(git -C "$repo" status --porcelain)" ] || die "$name has uncommitted changes; run ./deploy.sh push first"
    [ "$(git -C "$repo" branch --show-current)" = main ] || die "$name is not on main"
    git -C "$repo" fetch -q --tags
    git -C "$repo" rev-parse -q --verify "refs/tags/v$v" >/dev/null && die "$name already has tag v$v"
    [ "$(git -C "$repo" rev-list --count HEAD..@{u})" = 0 ] || die "$name is behind origin; pull first"
  done

  step "bump to $v"
  sed -i '' -E "1,/^version = /s/^version = \"[^\"]*\"/version = \"$v\"/" "$CAT/catalog/Cargo.toml" "$APP/app/Cargo.toml"
  sed -i '' -E "s#(<key>CFBundle(Short)?Version(String)?</key><string>)[^<]*#\1$v#" "$APP/app/bundle_mac.sh"
  sed -i '' -E "s/^(  \"version\": )\"[^\"]*\"/\1\"$v\"/" "$CAT/.claude-plugin/plugin.json" "$CAT/.codex-plugin/plugin.json"
  # Rewrites Cargo.lock for the new versions without upgrading any dependency.
  (cd "$CAT/catalog" && cargo metadata --format-version 1 >/dev/null)
  (cd "$APP/app" && cargo metadata --format-version 1 >/dev/null)
  # The hosted MCP Worker's tool contract carries the version too; CI fails if it is stale.
  (cd "$CAT/catalog" && cargo build -q --release --locked --bin biocartoon && python3 ../scripts/build_mcp_metadata.py >/dev/null)
  grep -q "\"version\": \"$v\"" "$CAT/worker/generated/mcp-metadata.json" || die "mcp-metadata.json: version not bumped"
  grep -m1 '^version' "$CAT/catalog/Cargo.toml" "$APP/app/Cargo.toml"
  grep -c "<string>$v</string>" "$APP/app/bundle_mac.sh" | grep -qx 2 || die "bundle_mac.sh: expected 2 version strings"
  for f in "$CAT/.claude-plugin/plugin.json" "$CAT/.codex-plugin/plugin.json"; do
    grep -qx "  \"version\": \"$v\"," "$f" || die "$f: version not bumped"
  done

  # The catalog repo goes first: the app's release build checks out biocartoon's main.
  for repo in "$CAT" "$APP"; do
    step "commit + tag $(basename "$repo") v$v"
    git -C "$repo" commit -qam "v$v"
    git -C "$repo" tag "v$v"
    if [ "$no_push" != --no-push ]; then
      git -C "$repo" push -q origin main
      git -C "$repo" push -q origin "v$v"
    fi
  done

  if [ "$no_push" = --no-push ]; then
    echo "committed and tagged locally; push with: git push origin main v$v (in both repos, biocartoon first)"
  else
    echo "PyPI:      gh run list -R josephrich98/biocartoon -L 3"
    echo "installers: gh run list -R josephrich98/biocartoon-app -L 3"
  fi
}

cmd="${1:-}"; shift || true
case "$cmd" in
  all)
    msg="${1:?usage: ./deploy.sh all \"MESSAGE\"}"
    brand; db; workers; web; site; desktop; plugin; push "$msg" ;;
  sync) sync_upstream "$@" ;;
  brand | db | workers | web | site | desktop) "$cmd" ;;
  plugin) plugin "$@" ;;
  push) push "$@" ;;
  release) release "$@" ;;
  *) sed -n '2,/^set -e/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
