#!/usr/bin/env python3
"""Fetch SteamSpy tag votes for the most-owned Steam apps, as input for build_genre_db.py.

SteamSpy asks for at most 1 request per second (appdetails) and 1 per minute (all), so this
takes about an hour for 4000 apps. CI runs it (.github/workflows/genre-db.yml); the result
is committed as data/steam_genres.tsv.

Usage: python3 tools/fetch_steamspy.py PAGES OUT.jsonl
  PAGES  number of request=all pages (1000 apps each, most owners first)
"""
import json
import sys
import time
import urllib.request

API = "https://steamspy.com/api.php"


def get(query, tries=4):
    for i in range(tries):
        try:
            req = urllib.request.Request(f"{API}?{query}", headers={"User-Agent": "leanfps-genre-db"})
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except Exception as e:  # network hiccup or rate limit: back off and retry
            print(f"{query}: {e}", file=sys.stderr)
            time.sleep(5 * (i + 1))
    return None


def main(pages, out):
    apps = []
    for page in range(pages):
        data = get(f"request=all&page={page}") or {}
        apps.extend(int(a) for a in data)
        print(f"page {page}: {len(data)} apps", file=sys.stderr)
        if page + 1 < pages:
            time.sleep(61)
    done = set()
    try:
        for line in open(out, encoding="utf-8"):
            done.add(json.loads(line)["appid"])
    except (OSError, ValueError, KeyError):
        pass
    with open(out, "a", encoding="utf-8") as f:
        for n, appid in enumerate(apps):
            if appid in done:
                continue
            d = get(f"request=appdetails&appid={appid}")
            if d and isinstance(d.get("tags"), dict):
                f.write(json.dumps({"appid": appid, "name": d.get("name"), "tags": d["tags"]}) + "\n")
            if n % 200 == 0:
                print(f"{n}/{len(apps)}", file=sys.stderr)
                f.flush()
            time.sleep(1.05)


if __name__ == "__main__":
    main(int(sys.argv[1]), sys.argv[2])
