#!/usr/bin/env python3
"""Writes English and Simplified Chinese into Localization/Localizable.xcstrings.

Every key the compiler extracted (scripts/check-strings.sh runs the sync) gets
an `en` unit (plural variations where a count is involved) and a `zh-Hans`
unit from translations_zh_hans.py. Exits non-zero when a key has no Chinese.
"""
import json
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ beside the scripts

here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, here)
from translations_zh_hans import ZH, EN_PLURALS  # noqa: E402

path = os.path.join(here, "..", "Localization", "Localizable.xcstrings")
with open(path, encoding="utf-8") as f:
    catalog = json.load(f)

# Keys the source no longer uses are dropped: the code is the source of truth.
stale = sorted(k for k, e in catalog["strings"].items() if e.get("extractionState") == "stale")
for key in stale:
    del catalog["strings"][key]

missing = []
for key, entry in catalog["strings"].items():
    localizations = {}
    if key in EN_PLURALS:
        localizations["en"] = {"variations": {"plural": {
            form: {"stringUnit": {"state": "translated", "value": value}}
            for form, value in EN_PLURALS[key].items()}}}
    else:
        localizations["en"] = {"stringUnit": {"state": "translated", "value": key}}
    if key in ZH:
        localizations["zh-Hans"] = {"stringUnit": {"state": "translated", "value": ZH[key]}}
    else:
        missing.append(key)
    entry["localizations"] = localizations

with open(path, "w", encoding="utf-8") as f:
    json.dump(catalog, f, ensure_ascii=False, indent=2, sort_keys=True)
    f.write("\n")

if stale:
    print("removed keys the source no longer uses:", *stale, sep="\n  ")
if missing:
    print("keys without a Simplified Chinese translation:", *missing, sep="\n  ")
    sys.exit(1)
print(f"{len(catalog['strings'])} keys, English and Simplified Chinese filled in")
