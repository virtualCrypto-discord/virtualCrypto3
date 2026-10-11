#!/usr/bin/env python3
"""Check a deployed release without credentials or business-data mutations."""

import argparse
import json
import time
import uuid
from html.parser import HTMLParser
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen


class Assets(HTMLParser):
    def __init__(self):
        super().__init__()
        self.paths = set()

    def handle_starttag(self, tag, attrs):
        for key, value in attrs:
            if key in ("src", "href") and value and value.startswith("/assets/"):
                self.paths.add(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("origin", help="HTTPS origin of the deployed app")
    args = parser.parse_args()
    origin = args.origin.rstrip("/")
    url = urlsplit(origin)
    if url.scheme != "https" or not url.netloc or url.path or url.query or url.fragment:
        parser.error("origin must be an HTTPS origin without a path, query or fragment")

    checks = []

    def request(path, expected, data=None):
        req = Request(
            origin + path,
            data=data,
            headers={"Accept": "application/json", "Content-Type": "application/json"},
        )
        try:
            response = urlopen(req, timeout=30)
        except HTTPError as error:
            response = error
        with response:
            body = response.read()
            if response.status != expected:
                raise RuntimeError(f"{path}: expected HTTP {expected}, got {response.status}")
            return response.headers, body

    # Wake a stopped app and its Flycast database before running the checks.
    deadline = time.monotonic() + 120
    while True:
        try:
            _, body = request("/health", 200)
            health = json.loads(body)
            if health.get("status") != "ok" or not health.get("version"):
                raise RuntimeError("/health: invalid response")
            break
        except (URLError, TimeoutError, RuntimeError):
            if time.monotonic() >= deadline:
                raise
            time.sleep(2)
    checks.append("health")

    headers, body = request("/", 200)
    if "text/html" not in headers.get("Content-Type", ""):
        raise RuntimeError("/: expected HTML")
    if "no-cache" not in headers.get("Cache-Control", ""):
        raise RuntimeError("/: expected Cache-Control: no-cache")
    assets = Assets()
    assets.feed(body.decode("utf-8"))
    if not assets.paths:
        raise RuntimeError("/: no built assets found")
    checks.append("spa")

    for path in sorted(assets.paths):
        headers, body = request(path, 200)
        if not body or "text/html" in headers.get("Content-Type", ""):
            raise RuntimeError(f"{path}: expected a built asset, not HTML or an empty body")
        if "immutable" not in headers.get("Cache-Control", ""):
            raise RuntimeError(f"{path}: expected immutable cache policy")
    checks.append(f"assets ({len(assets.paths)})")

    # This public lookup executes SQL; /health alone does not test the database.
    path = "/api/v2/currencies?name=release-smoke-" + uuid.uuid4().hex
    _, body = request(path, 404)
    if json.loads(body).get("error") != "not_found":
        raise RuntimeError("currency lookup: expected not_found")
    checks.append("database lookup")

    _, body = request("/api/v2/users/@me", 400)
    if json.loads(body).get("error") != "invalid_request":
        raise RuntimeError("users/@me: expected rejection without credentials")
    checks.append("authentication rejection")

    request("/api/integrations/discord/interactions", 401, b'{"type":1}')
    checks.append("unsigned Discord rejection")
    print(json.dumps({"origin": origin, "version": health["version"], "checks": checks}))


if __name__ == "__main__":
    main()
