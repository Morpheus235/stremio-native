#!/usr/bin/env python3
"""Build the .ipk — the `ar` container and both `.tar.gz` members — deterministically.

    python3 ci/mkipk.py --flavor stable --payload build/stage/stable/app \\
        --storage build/stremionative-storage --out build
    python3 ci/mkipk.py --emit-appinfo beta build/appinfo.json      (the descriptor alone)
    python3 ci/mkipk.py --verify stable build/<id>_<version>_arm.ipk (read a package back)
    python3 -I ci/mkipk.py --selftest

`--payload` is the application directory as the Makefile staged it: the binary `appinfo.json`'s
`main` names, the artwork, the fonts, the TLS roots, the bundled FFmpeg/libass libraries and the
licences. This writes, under one data root:

    usr/palm/applications/<id>/                    the payload, plus the flavour's appinfo.json
    usr/palm/packages/<id>/packageinfo.json        webOS's PACKAGE descriptor (which app ids and
                                                   services the package owns; without it an
                                                   install registers nothing)
    usr/palm/services/<id>.storage/                the private storage helper and services.json

and a control file synthesized from the same descriptor, so the version is single-sourced in
`pkg/appinfo.json` (through `ci/flavor.py`, which moves it for a beta). The output is
`<out>/<id>_<version>_arm.ipk`.

**Reproducible**: same inputs, same bytes. Every tar member's uid/gid/names/mtime/mode is
normalised and the members sorted; the gzip header carries mtime 0 and no file name; the `ar`
container is written here rather than by an archiver. The manifest sha256 a television verifies at
install is the whole integrity story of this distribution (nothing is code-signed), so a rebuild
must be distinguishable from a tampered artifact.

**Bare `ar` member names** (`debian-binary`, `control.tar.gz`, `data.tar.gz`, in that order). GNU
`ar` terminates short names with `/`, and LG's installer looks the members up by exact name and
fails the whole package (`error_code -5, "Failed to extract package"`). `Installed-Size` is KiB,
as opkg expects. The packager field names this file rather than impersonating LG's `ares-package`,
which is not reproducible.

Ported from PlxNative's `ci/mkipk.py` (GPL-3.0-or-later).
"""
from __future__ import annotations

import gzip
import io
import json
import os
import shutil
import sys
import tarfile
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True  # importing the sibling tools must not leave ci/__pycache__ behind
sys.path.insert(0, str(Path(__file__).resolve().parent))
import flavor  # noqa: E402  — ci/flavor.py decides a flavour's id, title and version

# Any fixed epoch works; 2010-01-01 is safely inside the range old opkg builds accept.
EPOCH = 1262304000
STORAGE_EXE = "stremionative-storage"
STORAGE_DESCRIPTION = "StremioNative private storage"
HOMEPAGE = "https://github.com/Morpheus235/stremio-native"
PACKAGER = "stremionative-mkipk/1"
# Every payload carries these, besides what appinfo.json names (`main`, the artwork).
REQUIRED = ["appfont.ttf", "appfont-bold.ttf", "appfont-cjk.ttf", "appfont-emoji.ttf", "OFL.txt", "le-roots.pem", "LICENSE", "NOTICE",
            "fonts/subs/manifest.json"]
# The bundled FFmpeg the player dlopens (crates/media/src/ff.rs). Without them the app starts,
# browses and refuses to play, so a package without them is not a package.
# libswresample-sn and libsnadec.so (native/adec.c) are the software audio decode -> PCM path.
REQUIRED_LIBS = ["libavutil-sn.so.61", "libswresample-sn.so.7", "libavcodec-sn.so.63", "libavformat-sn.so.63", "libsnadec.so"]
EXECUTABLE_NAMES = {STORAGE_EXE}


def die(msg: str) -> "None":
    raise SystemExit(f"mkipk: {msg}")


def is_arm_elf(path: Path) -> bool:
    head = path.read_bytes()[:20]
    return len(head) == 20 and head[:6] == b"\x7fELF\x01\x01" and head[18:20] == b"\x28\x00"


def check_payload(payload: Path, app: dict, require_arm: bool = True) -> None:
    """Fail clearly, naming what is missing, before anything is written."""
    if not payload.is_dir():
        die(f"payload directory {payload} does not exist")
    wanted = [app["main"]] + [app[k] for k in ("icon", "largeIcon", "splashBackground") if k in app] + REQUIRED
    missing = [n for n in wanted if not (payload / n).is_file()]
    if missing:
        die(f"payload {payload} is missing: {', '.join(missing)}")
    libs = [n for n in REQUIRED_LIBS if not (payload / n).is_file()]
    if libs:
        die(f"payload {payload} has no bundled FFmpeg ({', '.join(libs)}): build it with "
            "`make ffmpeg` (ci/build-ffmpeg.sh) or point BUNDLED_LIB_DIR at a directory holding the "
            "-sn libraries — see README.md, Building")
    if require_arm:
        for n in [app["main"]] + REQUIRED_LIBS:
            if not is_arm_elf(payload / n):
                die(f"{n} is not a little-endian 32-bit ARM ELF")
    if (payload / "appinfo.json").exists():
        die("the payload must not carry its own appinfo.json: the flavour's is written here")


def tar_info(name: str, size: int, mode: int, kind: bytes = tarfile.REGTYPE) -> tarfile.TarInfo:
    ti = tarfile.TarInfo(name)
    ti.size, ti.mode, ti.mtime, ti.type = size, mode, EPOCH, kind
    ti.uid = ti.gid = 0
    ti.uname = ti.gname = "root"
    return ti


def write_targz(entries: "dict[str, bytes | None]", executables: "set[str]") -> bytes:
    """A deterministic tar.gz of `entries` (arcname -> bytes, or None for a directory). Parent
    directories are added for every file; everything is sorted."""
    names = set(entries)
    for n in list(entries):
        parts = n.split("/")[:-1]
        for i in range(1, len(parts) + 1):
            names.add("/".join(parts[:i]))
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.GNU_FORMAT) as tf:
        for n in sorted(names):
            blob = entries.get(n)
            if blob is None:
                tf.addfile(tar_info(n, 0, 0o755, tarfile.DIRTYPE))
            else:
                mode = 0o755 if n.rsplit("/", 1)[-1] in executables else 0o644
                tf.addfile(tar_info(n, len(blob), mode), io.BytesIO(blob))
    out = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=out, mtime=0, compresslevel=9) as gz:
        gz.write(raw.getvalue())
    return out.getvalue()


def write_ar(members: "list[tuple[str, bytes]]") -> bytes:
    """A common-format `ar` archive, names verbatim — no trailing slash."""
    out = bytearray(b"!<arch>\n")
    for name, blob in members:
        assert len(name) <= 16, f"{name} needs the GNU long-name table this writer omits"
        out += f"{name:<16}{EPOCH:<12}{0:<6}{0:<6}{'100644':<8}{len(blob):<10}".encode() + b"\x60\n"
        out += blob
        if len(blob) % 2:
            out += b"\n"
    return bytes(out)


def packageinfo(app: dict) -> bytes:
    return (json.dumps({
        "app": app["id"],
        "services": [app["id"] + ".storage"],
        "requiredPermissions": app.get("requiredPermissions", []),
        "id": app["id"],
        "loc_name": app["title"],
        "package_format_version": 2,
        "vendor": app["vendor"],
        "version": app["version"],
    }, indent=4) + "\n").encode()


def services_json(app: dict) -> bytes:
    sid = app["id"] + ".storage"
    return (json.dumps({
        "id": sid, "description": STORAGE_DESCRIPTION, "engine": "native", "executable": STORAGE_EXE,
        "services": [{"name": sid, "commands": []}],
    }, indent=4) + "\n").encode()


def control(app: dict, installed_kib: int) -> bytes:
    lines = [
        f"Package: {app['id']}",
        f"Version: {app['version']}",
        "Section: misc",
        "Priority: optional",
        "Architecture: arm",
        f"Installed-Size: {installed_kib}",
        f"Maintainer: {app['vendor']}",
        f"Homepage: {HOMEPAGE}",
        "License: GPL-3.0-or-later",
        f"Description: {app.get('appDescription', app['title'])}",
        "webOS-Package-Format-Version: 2",
        f"webOS-Packager-Version: {PACKAGER}",
    ]
    return ("\n".join(lines) + "\n").encode()


def build(flav: str, payload: Path, storage: Path, out_dir: Path, date: "str | None" = None,
          require_arm: bool = True) -> Path:
    app = flavor.appinfo_for(flav, date)
    check_payload(payload, app, require_arm)
    if not storage.is_file() or storage.is_symlink():
        die(f"storage helper {storage} is missing; build it with `make` (crates/storage)")
    if require_arm and not is_arm_elf(storage):
        die(f"{storage} is not a little-endian 32-bit ARM ELF")
    app_dir = f"usr/palm/applications/{app['id']}"
    svc_dir = f"usr/palm/services/{app['id']}.storage"
    data: "dict[str, bytes | None]" = {}
    for p in sorted(payload.rglob("*")):
        rel = p.relative_to(payload).as_posix()
        if p.is_symlink():
            die(f"payload {rel} is a symlink; stage the file itself")
        data[f"{app_dir}/{rel}"] = p.read_bytes() if p.is_file() else None
    data[f"{app_dir}/appinfo.json"] = (json.dumps(app, indent=2) + "\n").encode()
    data[f"usr/palm/packages/{app['id']}/packageinfo.json"] = packageinfo(app)
    data[f"{svc_dir}/services.json"] = services_json(app)
    data[f"{svc_dir}/{STORAGE_EXE}"] = storage.read_bytes()
    executables = EXECUTABLE_NAMES | {app["main"]}
    kib = (sum(len(b) for b in data.values() if b is not None) + 1023) // 1024
    data_tgz = write_targz(data, executables)
    control_tgz = write_targz({"control": control(app, kib)}, set())
    ipk = write_ar([("debian-binary", b"2.0\n"), ("control.tar.gz", control_tgz), ("data.tar.gz", data_tgz)])
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / f"{app['id']}_{app['version']}_arm.ipk"
    tmp = out.with_suffix(".ipk.tmp")
    tmp.write_bytes(ipk)
    os.replace(tmp, out)
    print(f"wrote {out} ({len(ipk)} bytes) — {app['id']} {app['version']}, Installed-Size {kib} KiB")
    return out


def verify(ipk: Path, flav: str, date: "str | None" = None) -> "list[str]":
    """Read a built package back the way an installer does and list what is wrong with it."""
    import hbrepo  # ci/hbrepo.py — the Homebrew Channel's own reading of the control file

    app = flavor.appinfo_for(flav, date)
    errors = []
    blob = ipk.read_bytes()
    try:
        members = hbrepo.ar_members(blob)
        ctl = hbrepo.control_of(blob)
    except ValueError as e:
        return [f"unreadable package: {e}"]
    if list(members) != ["debian-binary", "control.tar.gz", "data.tar.gz"]:
        errors.append(f"ar members {list(members)}")
    if ctl.get("Package") != app["id"] or ctl.get("Version") != app["version"]:
        errors.append(f"control names {ctl.get('Package')} {ctl.get('Version')}, the flavour is {app['id']} {app['version']}")
    if ipk.name != f"{app['id']}_{app['version']}_arm.ipk":
        errors.append(f"file name {ipk.name}")
    root = f"usr/palm/applications/{app['id']}/"
    with tarfile.open(fileobj=io.BytesIO(members["data.tar.gz"])) as tf:
        infos = {m.name: m for m in tf.getmembers()}
        need = [app["main"], "appinfo.json", app["icon"], app["largeIcon"], app["splashBackground"]] + REQUIRED + REQUIRED_LIBS
        errors += [f"missing {root}{n}" for n in need if root + n not in infos]
        if root + "appinfo.json" in infos and json.load(tf.extractfile(infos[root + "appinfo.json"])) != app:
            errors.append("the packaged appinfo.json is not the flavour's")
        main = infos.get(root + app["main"])
        if main is not None and (main.mode != 0o755 or tf.extractfile(main).read(20)[18:20] != b"\x28\x00"):
            errors.append("the binary is not an executable ARM ELF")
        svc = f"usr/palm/services/{app['id']}.storage/{STORAGE_EXE}"
        if svc not in infos:
            errors.append(f"missing {svc}")
        stray = sorted({n.split("/")[3] for n in infos if n.startswith("usr/palm/applications/") and n.count("/") >= 3} - {app["id"]})
        if stray:
            errors.append(f"other installs' directories: {stray}")
    return errors


# ---- selftest ------------------------------------------------------------------------------------

def _fake_arm_elf(tag: bytes) -> bytes:
    head = bytearray(52)
    head[:6] = b"\x7fELF\x01\x01"
    head[16:18] = (2).to_bytes(2, "little")
    head[18:20] = b"\x28\x00"
    return bytes(head) + tag


def _payload(root: Path, app: dict) -> Path:
    p = root / "payload"
    (p / "fonts/subs").mkdir(parents=True)
    (p / app["main"]).write_bytes(_fake_arm_elf(b"app"))
    for n in REQUIRED_LIBS:
        (p / n).write_bytes(_fake_arm_elf(n.encode()))
    for n in REQUIRED + [app["icon"], app["largeIcon"], app["splashBackground"]]:
        (p / n).write_bytes(n.encode())
    return p


def _selftest() -> int:
    import hbrepo  # ci/hbrepo.py: the Homebrew Channel reader must read what this writes

    fails = []

    def check(cond: bool, msg: str) -> None:
        print(f"  {'ok  ' if cond else 'FAIL'} — {msg}")
        if not cond:
            fails.append(msg)

    tracked = flavor.tracked_appinfo()
    for key in ("icon", "largeIcon", "splashBackground", "main"):
        check(key in tracked, f"pkg/appinfo.json names its {key}")
    tmp = Path(tempfile.mkdtemp())
    try:
        payload = _payload(tmp, tracked)
        storage = tmp / STORAGE_EXE
        storage.write_bytes(_fake_arm_elf(b"storage"))
        a = build("stable", payload, storage, tmp / "a")
        b = build("stable", payload, storage, tmp / "b")
        check(a.name == f"{tracked['id']}_{tracked['version']}_arm.ipk", f"named <id>_<version>_arm.ipk ({a.name})")
        check(a.read_bytes() == b.read_bytes(), "two builds of the same inputs are byte-identical")
        blob = a.read_bytes()
        members = hbrepo.ar_members(blob)
        check(list(members) == ["debian-binary", "control.tar.gz", "data.tar.gz"], f"ar members in order ({list(members)})")
        check(b"debian-binary/" not in blob[:200] and blob[8:24] == b"debian-binary   ", "bare ar member names (no GNU slash)")
        check(members["debian-binary"] == b"2.0\n", "debian-binary is 2.0")
        check(members["control.tar.gz"][4:8] == b"\0\0\0\0" and members["data.tar.gz"][4:8] == b"\0\0\0\0", "gzip headers carry mtime 0")
        ctl = hbrepo.control_of(blob)
        check(ctl.get("Package") == tracked["id"] and ctl.get("Version") == tracked["version"], f"control names the package ({ctl})")
        check(ctl.get("Architecture") == "arm" and ctl.get("webOS-Package-Format-Version") == "2", "control architecture and format")
        check(int(ctl.get("Installed-Size", "0")) > 0, "control carries a real Installed-Size")
        with tarfile.open(fileobj=io.BytesIO(members["data.tar.gz"])) as tf:
            infos = {m.name: m for m in tf.getmembers()}
            app_dir = f"usr/palm/applications/{tracked['id']}"
            appinfo = json.load(tf.extractfile(infos[f"{app_dir}/appinfo.json"]))
            check(appinfo == tracked, "the stable package carries the tracked appinfo.json unchanged")
            check(infos[f"{app_dir}/{tracked['main']}"].mode == 0o755, "the binary is executable")
            svc = f"usr/palm/services/{tracked['id']}.storage"
            check(infos[f"{svc}/{STORAGE_EXE}"].mode == 0o755, "the storage helper is executable")
            check(json.load(tf.extractfile(infos[f"{svc}/services.json"]))["executable"] == STORAGE_EXE, "services.json names the helper")
            pinfo = json.load(tf.extractfile(infos[f"usr/palm/packages/{tracked['id']}/packageinfo.json"]))
            check(pinfo["app"] == tracked["id"] and pinfo["services"] == [tracked["id"] + ".storage"], "packageinfo owns the app and its service")
            check(all(m.uid == 0 and m.gid == 0 and m.mtime == EPOCH and m.uname == "root" for m in infos.values()),
                  "every member is root-owned at the fixed epoch")
            check(infos[f"{app_dir}/icon.png"].mode == 0o644, "data files are 0644")
            check(f"{app_dir}/{REQUIRED_LIBS[0]}" in infos and f"{app_dir}/fonts/subs/manifest.json" in infos,
                  "the bundled libraries and the subtitle fonts are packaged")
        check(verify(a, "stable") == [], f"the stable package verifies ({verify(a, 'stable')})")
        check(verify(a, "beta", "202610161347") != [], "it does not verify as the beta")

        beta = build("beta", payload, storage, tmp / "beta", date="202610161347")
        x, y, _ = tracked["version"].split(".")
        bctl = hbrepo.control_of(beta.read_bytes())
        check(beta.name == f"{tracked['id']}.beta_{x}.{y}.261016134_arm.ipk", f"a beta is its own app at X.Y.<cut> ({beta.name})")
        check(bctl.get("Package") == f"{tracked['id']}.beta" and bctl.get("Version") == f"{x}.{y}.261016134", f"beta control ({bctl})")
        with tarfile.open(fileobj=io.BytesIO(hbrepo.ar_members(beta.read_bytes())["data.tar.gz"])) as tf:
            names = [m.name for m in tf.getmembers()]
            bapp = json.load(tf.extractfile(f"usr/palm/applications/{tracked['id']}.beta/appinfo.json"))
            check(bapp["title"] == f"{tracked['title']} Beta", "the beta's tile says Beta")
            check(not any(f"applications/{tracked['id']}/" in n for n in names), "a beta package carries no stable directory")
        check(verify(beta, "beta", "202610161347") == [], f"the beta package verifies ({verify(beta, 'beta', '202610161347')})")

        (payload / REQUIRED_LIBS[1]).unlink()
        try:
            build("stable", payload, storage, tmp / "c")
        except SystemExit as e:
            check("bundled FFmpeg" in str(e) and REQUIRED_LIBS[1] in str(e), f"a payload without FFmpeg fails naming it ({e})")
        else:
            check(False, "a payload without FFmpeg fails naming it")
        (payload / REQUIRED_LIBS[1]).write_bytes(_fake_arm_elf(b"x"))
        (payload / tracked["main"]).write_bytes(b"#!/bin/sh\n")
        try:
            build("stable", payload, storage, tmp / "d")
        except SystemExit as e:
            check("ARM ELF" in str(e), f"a binary that is not ARM is refused ({e})")
        else:
            check(False, "a binary that is not ARM is refused")
        (payload / tracked["main"]).write_bytes(_fake_arm_elf(b"app"))
        (payload / "appinfo.json").write_text("{}")
        try:
            build("stable", payload, storage, tmp / "e")
        except SystemExit:
            check(True, "a payload carrying its own appinfo.json is refused")
        else:
            check(False, "a payload carrying its own appinfo.json is refused")
    finally:
        shutil.rmtree(tmp)
    print()
    for f in fails:
        print(f"::error::{f}")
    return 1 if fails else 0


def main(argv: "list[str]") -> int:
    if argv == ["--selftest"]:
        print("== ipk packager ==")
        return _selftest()
    if argv[:1] == ["--verify"] and len(argv) == 3:
        errors = verify(Path(argv[2]), argv[1])
        for e in errors:
            print(f"::error::{argv[2]}: {e}")
        if not errors:
            print(f"verified {argv[2]} as the {argv[1]} install")
        return 1 if errors else 0
    if argv[:1] == ["--emit-appinfo"] and len(argv) == 3:
        out = Path(argv[2])
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(flavor.appinfo_for(argv[1]), indent=2) + "\n")
        print(f"wrote {out} — id={flavor.app_id(argv[1])}")
        return 0
    import argparse
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--flavor", default=os.environ.get("FLAVOR", "stable"))
    p.add_argument("--payload", required=True, type=Path)
    p.add_argument("--storage", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    a = p.parse_args(argv)
    build(a.flavor, a.payload, a.storage, a.out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
