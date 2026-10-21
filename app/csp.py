"""Trunk post-build hook (Trunk.toml): security headers for app.biocartoon.com.

Each page in the staging dir gets a Content-Security-Policy <meta> listing the SHA-256 of its own
inline scripts, so nothing else can run there even if text from a project, catalog or account ever
reached the page as HTML. Trunk's inline loader changes with every build, which is why this runs
after the build rather than living in index.html. The headers a <meta> can't set go in _headers,
which Cloudflare's static assets apply to every response.

Keep connect-src in step with the hosts the app fetches from: DEFAULT_ASSET_URL (biocartoon
catalog/src/lib.rs), DEFAULT_SUBMIT_URL (src/submit.rs) and the AI providers (src/ai.rs). A custom
asset or submission URL set in Settings is blocked on the web build, as is a vLLM server anywhere
but localhost.
"""
import base64
import hashlib
import os
import re
from pathlib import Path

STAGING = Path(os.environ["TRUNK_STAGING_DIR"])
DEBUG = os.environ.get("TRUNK_PROFILE") != "release"

APP_HOSTS = [
    "https://assets.biocartoon.com",
    "https://api.biocartoon.com",
    "https://api.openai.com",
    "https://generativelanguage.googleapis.com",
    "https://api.anthropic.com",
]
# A vLLM server on the user's own machine (src/ai.rs); under trunk serve, also a local Worker
# (wrangler dev).
LOCAL_HOSTS = ["http://localhost:*", "http://127.0.0.1:*"]
# trunk serve: trunk's own reload socket.
DEV_HOSTS = ["ws://localhost:*", "ws://127.0.0.1:*"] if DEBUG else []

# 'wasm-unsafe-eval' lets the page compile its own .wasm; it doesn't allow eval() of strings.
POLICIES = {
    "index.html": {
        "script-src": ["'self'", "'wasm-unsafe-eval'"],
        "connect-src": ["'self'", *APP_HOSTS, *LOCAL_HOSTS, *DEV_HOSTS],
        "img-src": ["'self'", "data:", "blob:"],
    },
    "download.html": {
        "script-src": [],
        "connect-src": ["https://api.github.com"],
        "img-src": ["'self'", "data:"],
    },
}
COMMON = {
    "default-src": ["'none'"],
    "style-src": ["'self'", "'unsafe-inline'"],
    "manifest-src": ["'self'"],
    "base-uri": ["'none'"],
    "form-action": ["'none'"],
}

HEADERS = """/*
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
  Referrer-Policy: no-referrer
  Strict-Transport-Security: max-age=31536000
  Cross-Origin-Opener-Policy: same-origin
  Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()
"""

INLINE_SCRIPT = re.compile(r"<script(?![^>]*\bsrc=)[^>]*>(.*?)</script>", re.S | re.I)
CHARSET = re.compile(r'<meta charset="utf-8"\s*/?>', re.I)


def policy(name: str, html: str) -> str:
    p = {**COMMON, **POLICIES[name]}
    hashes = [f"'sha256-{base64.b64encode(hashlib.sha256(m.encode()).digest()).decode()}'" for m in INLINE_SCRIPT.findall(html)]
    p["script-src"] = p["script-src"] + hashes or ["'none'"]
    return "; ".join(f"{k} {' '.join(v)}" for k, v in p.items())


for name in POLICIES:
    path = STAGING / name
    html = path.read_text()
    if "Content-Security-Policy" in html:
        raise SystemExit(f"csp.py: {name} already has a Content-Security-Policy")
    # The meta has to come before the first script, so it goes right after <meta charset>.
    if not CHARSET.search(html):
        raise SystemExit(f'csp.py: {name} needs <meta charset="utf-8" /> for the policy to follow')
    meta = f'<meta http-equiv="Content-Security-Policy" content="{policy(name, html)}" />'
    path.write_text(CHARSET.sub(lambda m: f"{m.group(0)}\n  {meta}", html, count=1))
(STAGING / "_headers").write_text(HEADERS)
