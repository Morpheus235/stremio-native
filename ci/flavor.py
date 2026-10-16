#!/usr/bin/env python3
"""Which install a package is for: a transform from the tracked `pkg/appinfo.json` to a flavour's.

Three installs of this app can live on one television, each with its own app id, launcher tile,
sign-in and `/tmp` root (`crates/base/src/paths.rs` reads the id back from the install
directory's name):

  * `stable` — what users install (`com.morpheus235.stremionative`). Always a `RELEASE=1` build.
  * `beta`   — the prerelease channel beside it (`…stremionative.beta`, tile "StremioNative Beta"),
               also always `RELEASE=1`. Its PACKAGE version is `X.Y.YYYYMMDD`: the tracked `X.Y`
               with the UTC cut date as the patch, because Homebrew Channel offers an update only
               when the version string differs and webOS takes nothing but three integers, so the
               date is what orders betas (one per day).
  * `debug`  — the developer build (`…stremionative.debug`, tile "StremioNative debug").

The install list lives in `ci/install-identities.json`, which the storage layer's build script
also reads; this file decides what else moves.

**PATCH, DO NOT DUPLICATE.** Only `id` and `title` move for `debug`, and `id`, `title` and
`version` for `beta`. Everything else in the descriptor (`main`, `transparent`, `requiredMemory`,
the icon file NAMES, ...) is behaviour and must never drift between installs; a second checked-in
descriptor would drift the first time one was edited. The badged artwork is staged over the same
basenames from `pkg/<flavour>/`, so the flavour lives in the directory a file is read FROM, never
in the name it is packaged UNDER.

**THE STABLE TRANSFORM IS THE IDENTITY**, and `--selftest` asserts it: the released descriptor is
byte-for-byte the tracked one.

    python3 ci/flavor.py --list                 stable debug beta
    python3 ci/flavor.py beta                   the app id
    python3 ci/flavor.py --version beta         the package version (beta reads SN_BETA_DATE)
    python3 ci/flavor.py --title beta           the launcher title
    python3 -I ci/flavor.py --selftest
"""
from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

INSTALL_IDENTITIES = json.loads((ROOT / "ci/install-identities.json").read_text())
FLAVORS = tuple(identity["name"] for identity in INSTALL_IDENTITIES)
STABLE_ID = next(i["app_id"] for i in INSTALL_IDENTITIES if i["name"] == "stable")
# The launcher titles of the flavoured installs (the beta's is a product name, capitalised; the
# developer build's is the bare suffix).
TITLE_SUFFIX = {"debug": "debug", "beta": "Beta"}


def tracked_appinfo() -> dict:
    return json.loads((ROOT / "pkg/appinfo.json").read_text())


def app_id(flavor: str) -> str:
    if flavor not in FLAVORS:
        raise SystemExit(f"unknown flavour {flavor!r} — one of: {', '.join(FLAVORS)}")
    return next(i["app_id"] for i in INSTALL_IDENTITIES if i["name"] == flavor)


def beta_date(date: "str | None" = None) -> str:
    """The `YYYYMMDD` a beta is cut for: the argument, else `SN_BETA_DATE` (which the Makefile
    exports, so the package version and the version the binary reports name the same day). There
    is no fallback to "today": a guessed date would make the package disagree with its binary."""
    d = date if date is not None else os.environ.get("SN_BETA_DATE", "")
    if not re.fullmatch(r"[0-9]{8}", d) or not ("19700101" <= d <= "99991231") or not (1 <= int(d[4:6]) <= 12) \
            or not (1 <= int(d[6:]) <= 31):
        raise SystemExit(f"a beta package's version carries its cut date: pass it or set SN_BETA_DATE=YYYYMMDD (got {d!r})")
    return d


def version_for(flavor: str, date: "str | None" = None) -> str:
    v = tracked_appinfo()["version"]
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", v)
    if not m:
        raise SystemExit(f"pkg/appinfo.json version {v!r} is not X.Y.Z")
    if flavor == "beta":
        return f"{m.group(1)}.{m.group(2)}.{int(beta_date(date))}"
    app_id(flavor)
    return v


def appinfo_for(flavor: str, date: "str | None" = None) -> dict:
    """The tracked descriptor re-pointed at `flavor`; the identity for stable."""
    a = dict(tracked_appinfo())
    if flavor == "stable":
        return a
    a["id"] = app_id(flavor)
    a["title"] = f"{a['title']} {TITLE_SUFFIX[flavor]}"
    if flavor == "beta":
        a["version"] = version_for(flavor, date)
    return a


def _selftest() -> int:
    fails = []

    def check(cond: bool, msg: str) -> None:
        print(f"  {'ok  ' if cond else 'FAIL'} — {msg}")
        if not cond:
            fails.append(msg)

    tracked = tracked_appinfo()
    check(appinfo_for("stable") == tracked, "appinfo_for('stable') is the identity — the released descriptor cannot move")
    check(tracked["id"] == STABLE_ID, f"pkg/appinfo.json id is the stable install's ({STABLE_ID})")
    check(set(FLAVORS) == {"stable", "debug", "beta"}, f"the installs are stable, debug and beta ({FLAVORS})")
    for name in FLAVORS:
        if name != "stable":
            check(app_id(name) == f"{STABLE_ID}.{name}", f"{name} id is {STABLE_ID}.{name}")

    dbg = appinfo_for("debug")
    check(dbg["title"] == f"{tracked['title']} debug", f"debug title ({dbg['title']!r})")
    moved = {k for k in tracked if dbg.get(k) != tracked[k]}
    check(moved == {"id", "title"}, f"only id and title differ for debug (saw {sorted(moved)})")

    beta = appinfo_for("beta", "20261016")
    x, y, _ = tracked["version"].split(".")
    check(beta["title"] == f"{tracked['title']} Beta", f"beta title is \"{tracked['title']} Beta\" ({beta['title']!r})")
    check(beta["version"] == f"{x}.{y}.20261016", f"beta version is X.Y.<date> ({beta['version']!r})")
    check(re.fullmatch(r"\d+\.\d+\.\d+", beta["version"]) is not None, "beta version is three integers — all webOS accepts")
    later = appinfo_for("beta", "20261017")
    check(int(later["version"].rsplit(".", 1)[1]) > int(beta["version"].rsplit(".", 1)[1]),
          "a later cut is a later version (Homebrew Channel offers an update only when the string differs)")
    moved = {k for k in tracked if beta.get(k) != tracked[k]}
    check(moved == {"id", "title", "version"}, f"only id, title and version differ for beta (saw {sorted(moved)})")
    for bad in ("2026101", "202610160", "2026-10-1", "", "abcdefgh", "20261316", "20261000"):
        try:
            appinfo_for("beta", bad)
        except SystemExit:
            check(True, f"beta refuses a malformed date {bad!r}")
        else:
            check(False, f"beta refuses a malformed date {bad!r}")
    saved = os.environ.pop("SN_BETA_DATE", None)
    try:
        try:
            appinfo_for("beta")
        except SystemExit:
            check(True, "beta with no date and no SN_BETA_DATE is an error, never a guess")
        else:
            check(False, "beta with no date and no SN_BETA_DATE is an error, never a guess")
        os.environ["SN_BETA_DATE"] = "20261016"
        check(appinfo_for("beta") == beta, "SN_BETA_DATE (what the Makefile exports) supplies the date")
    finally:
        os.environ.pop("SN_BETA_DATE", None)
        if saved is not None:
            os.environ["SN_BETA_DATE"] = saved
    check(version_for("stable", "20261016") == tracked["version"] and version_for("debug") == tracked["version"],
          "a date changes nothing for stable and debug")
    try:
        app_id("stabel")
    except SystemExit:
        check(True, "an unknown flavour is refused")
    else:
        check(False, "an unknown flavour is refused")

    # The same facts, in the languages that cannot read this file.
    paths = (ROOT / "crates/base/src/paths.rs").read_text()
    check(f'STABLE_APP_ID: &str = "{STABLE_ID}"' in paths, "crates/base/src/paths.rs STABLE_APP_ID agrees")
    mk_path = ROOT / "Makefile"
    if mk_path.is_file():
        mk = mk_path.read_text()
        check(re.search(rf"(?m)^APPID_STABLE\s*:?=\s*{re.escape(STABLE_ID)}\s*$", mk) is not None, "Makefile APPID_STABLE agrees")
        check(re.search(r"(?m)^FLAVORS\s*:=\s*\$\(shell python3 ci/flavor\.py --list\)\s*$", mk) is not None,
              "Makefile reads the flavour list from here")
        goal = re.search(r"(?m)^\.DEFAULT_GOAL\s*:?=\s*(\S+)", mk)
        check(goal is not None and goal.group(1) == "all", "a bare `make` builds the binary (default goal `all`)")
    hb = (ROOT / "ci/hbrepo.py").read_text()
    check(f'TITLE = "{tracked["title"]}"' in hb, "ci/hbrepo.py lists the stable title")
    rel = (ROOT / ".github/workflows/hbrepo.yml").read_text()
    check(f'channel true "{appinfo_for("beta", "20261016")["title"]}"' in rel, "the Homebrew Channel workflow lists the beta title")

    print()
    for f in fails:
        print(f"::error::{f}")
    return 1 if fails else 0


def main(argv: "list[str]") -> int:
    if argv == ["--list"]:
        print(" ".join(FLAVORS))
        return 0
    if argv == ["--selftest"]:
        print("== flavour transform ==")
        return _selftest()
    if len(argv) == 2 and argv[0] == "--version":
        print(version_for(argv[1]))
        return 0
    if len(argv) == 2 and argv[0] == "--title":
        app_id(argv[1])
        print(appinfo_for(argv[1])["title"] if argv[1] != "beta" else f"{tracked_appinfo()['title']} {TITLE_SUFFIX['beta']}")
        return 0
    if len(argv) == 1 and not argv[0].startswith("-"):
        print(app_id(argv[0]))
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
