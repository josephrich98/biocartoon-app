#!/usr/bin/env bash
# Deploy BioCartoon. Expects the biocartoon and biocartoon-app checkouts side by side.
#
#   ./deploy.sh all "MESSAGE"     db + workers + web + site + desktop, then push both repos
#   ./deploy.sh sync [--apply]    new upstream icons -> master DB (dry run without --apply; --apply also runs db)
#   ./deploy.sh db                master DB -> R2 (also merges approved app uploads, rewrites both READMEs)
#   ./deploy.sh workers           D1 migrations, then the submit + review Workers
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

sync_upstream() {
  step "sync upstream icons ($*)"
  (cd "$CAT" && python3 data/sync.py "$@")
}

db() {
  step "publish catalog to R2"
  (cd "$CAT" && python3 data/publish_catalog.py)
}

workers() {
  step "D1 migrations + Workers"
  (cd "$CAT/worker" && npm run migrate && npm test && npm run deploy)
}

web() {
  step "web app"
  # trunk replaces dist/, which also drops the BioCartoon.app that bundle_mac.sh leaves there.
  (cd "$APP/app" && trunk build --release && npx wrangler deploy)
}

site() {
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
  # Rewrites Cargo.lock for the new versions without upgrading any dependency.
  (cd "$CAT/catalog" && cargo metadata --format-version 1 >/dev/null)
  (cd "$APP/app" && cargo metadata --format-version 1 >/dev/null)
  grep -m1 '^version' "$CAT/catalog/Cargo.toml" "$APP/app/Cargo.toml"
  grep -c "<string>$v</string>" "$APP/app/bundle_mac.sh" | grep -qx 2 || die "bundle_mac.sh: expected 2 version strings"

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
    db; workers; web; site; desktop; push "$msg" ;;
  sync) sync_upstream "$@" ;;
  db | workers | web | site | desktop) "$cmd" ;;
  push) push "$@" ;;
  release) release "$@" ;;
  *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
