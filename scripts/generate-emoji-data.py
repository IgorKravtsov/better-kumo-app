#!/usr/bin/env python3
"""Regenerate core/crates/kumo-core/src/chat/emoji.tsv: `group<TAB>emoji<TAB>labels<TAB>tags`,
names in en/uk/ru separated by `|`.

Keys come from Unicode's emoji-test.txt, not emojibase: emojibase adds U+FE0F to emoji like 👍,
which other clients send without it, so the reactions would not group. UNICODE_EMOJI 15.0 is the
newest set iOS 17 renders.

Usage: scripts/generate-emoji-data.py
"""
import json
import pathlib
import urllib.request

VERSION = "16.0.3"
UNICODE_EMOJI = "15.0"
LOCALES = ["en", "uk", "ru"]
COMPONENT_GROUP = 2
OUT = pathlib.Path(__file__).resolve().parent.parent / "core/crates/kumo-core/src/chat/emoji.tsv"


def fetch(path):
    url = f"https://cdn.jsdelivr.net/npm/emojibase-data@{VERSION}/{path}"
    with urllib.request.urlopen(url) as r:
        return json.load(r)


def fully_qualified():
    url = f"https://unicode.org/Public/emoji/{UNICODE_EMOJI}/emoji-test.txt"
    with urllib.request.urlopen(url) as r:
        text = r.read().decode("utf-8")
    out = {}
    for line in text.splitlines():
        if "; fully-qualified" not in line:
            continue
        points = line.split(";")[0].split()
        key = "-".join(p for p in points if p != "FE0F")
        out[key] = "".join(chr(int(p, 16)) for p in points)
    return out


def clean(words):
    out = []
    for w in words:
        w = w.lower().replace("|", " ").replace("\t", " ").replace("’", "'").replace("ʼ", "'").strip()
        if w and w not in out:
            out.append(w)
    return out


def main():
    qualified = fully_qualified()
    full = fetch("en/data.json")
    local = {loc: {e["hexcode"]: e for e in fetch(f"{loc}/compact.json")} for loc in LOCALES}
    lines = []
    for e in sorted(full, key=lambda e: e.get("order", 1 << 30)):
        emoji = qualified.get("-".join(p for p in e["hexcode"].split("-") if p != "FE0F"))
        if emoji is None or e.get("group") is None or e["group"] == COMPONENT_GROUP:
            continue
        labels, tags = [], []
        for loc in LOCALES:
            item = local[loc].get(e["hexcode"])
            if item:
                labels.append(item["label"])
                tags.extend(item.get("tags", []))
        lines.append(f"{e['group']}\t{emoji}\t{'|'.join(clean(labels))}\t{'|'.join(clean(tags))}")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{len(lines)} emoji written to {OUT}")


if __name__ == "__main__":
    main()
