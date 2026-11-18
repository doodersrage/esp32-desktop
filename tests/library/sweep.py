#!/usr/bin/env python3
"""Build every esp32bench.dev library project for every board it lists, and boot each
ESP32, ESP32-S3 and ESP32-C3 build in the simulator: the v1 check in docs/PLAN.md
("every library project builds ... and every ESP32, S3 and C3 project boots in the
simulator").

It drives the CLI (`esp32bench build`, `esp32bench sim run`), so it checks what the app
does. What each project should print comes from the site's lab expectations
(esp32-bench `src/data/lab-expect.ts`), exported as JSON:

    cd <esp32-bench> && node --experimental-strip-types -e \\
      "import('./src/data/lab-expect.ts').then(m => console.log(JSON.stringify(m.LAB_EXPECT)))" \\
      > expect.json

Boot results:
  pass        printed the project's expected output, no crash
  started     ran without crashing but did not print it (often: it needs Wi-Fi or
              parts that the simulation does not have; see `needs`)
  crash       a panic, abort or reboot
  no-image    the build produced no image to boot
  started (console on USB, not simulated)
              a board whose Serial is the chip's own USB, which QEMU does not model

Prebuilt projects (firmware ships ready-made) are listed, not built or booted.

Usage: sweep.py --cli core/target/debug/esp32bench --expect expect.json --out DIR
       [--only ID ...] [--skip-build] [--jobs N]
"""

import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

# Boards the plan keeps workbench-only (built, not simulated), whatever their chip.
WORKBENCH_ONLY = {"xiao-esp32s3", "xiao-esp32c3", "esp32-cam"}
SIMULATED_CHIPS = {"esp32", "esp32s3", "esp32c3"}
# Boards whose builds send Serial over the chip's own USB (USB CDC on boot): they have
# no USB-UART chip. The simulator shows UART0 only, so their output is not seen.
USB_CONSOLE = {"esp32c3-supermini"}
CRASH = [
    re.compile(p)
    for p in (
        r"Guru Meditation Error",
        r"abort\(\) was called",
        r"\bpanic'ed\b",
        r"assert failed:",
        r"Brownout detector was triggered",
        r"Stack canary watchpoint triggered",
        r"Task watchdog got triggered",
    )
]
# Each boot prints one ROM reset line ("rst:0x1 (POWERON)..."); more is a reboot.
RESET = re.compile(r"\brst:0x[0-9a-f]+")


def run(cmd, timeout, env=None):
    t = time.monotonic()
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=timeout, env=env)
        return p.returncode, p.stdout + p.stderr, time.monotonic() - t
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b"") + (e.stderr or b"")
        return None, out, time.monotonic() - t


def build(cli, project, board):
    code, out, secs = run([cli, "build", project, "--board", board, "--yes"], 1800)
    text = out.decode("utf-8", "replace")
    tail = [l for l in text.splitlines() if l.strip()][-6:]
    return {"ok": code == 0, "seconds": round(secs), "tail": tail}


def image_for(project, board):
    out = Path.home() / ".cache/esp32bench/builds" / project / board / "out"
    found = sorted(out.glob("*.merged.bin")) if out.is_dir() else []
    return found[0] if found else None


def boot(cli, project, board, chip, expect):
    img = image_for(project, board)
    if img is None:
        return {"result": "no-image"}
    e = expect.get(project, {})
    seconds = max(int(e.get("seconds", 10)), 8) + 6  # time to boot the ROM and app
    # The console ends when stdin closes, so feed it a silent `sleep` and let the
    # timeout stop it. Arguments go through "$@", never into the script text.
    code, out, _ = run(
        ["bash", "-c", 'sleep "$1" | timeout --signal=INT "$1" "$2" sim run --chip "$3" "$4"',
         "sweep", str(seconds), cli, chip, str(img)],
        seconds + 30,
    )
    text = out.decode("utf-8", "replace")
    crashed = next((c.pattern for c in CRASH if c.search(text)), None)
    resets = len(RESET.findall(text))
    if crashed is None and resets > 1:
        crashed = f"rebooted ({resets} resets)"
    patterns = e.get("patterns", [])
    matched = bool(patterns) and all(re.search(p, text) for p in patterns)
    result = "crash" if crashed else ("pass" if matched else "started")
    if result == "started" and board in USB_CONSOLE:
        result = "started (console on USB, not simulated)"
    lines = [l for l in text.splitlines() if l.strip()]
    return {
        "result": result,
        "crash": crashed,
        "needs": e.get("needs"),
        "engine": next((l.split()[-1] for l in lines if l.startswith("engine")), None),
        "tail": lines[-8:],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cli", required=True)
    ap.add_argument("--expect", required=True)
    ap.add_argument("--manifest", default=str(Path.home() / ".cache/esp32bench/desktop-manifest.json"))
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--skip-build", action="store_true")
    ap.add_argument("--jobs", type=int, default=4, help="simulations at once")
    a = ap.parse_args()

    m = json.load(open(a.manifest))
    expect = json.load(open(a.expect))
    chip_of = {b["id"]: b["chip"] for b in m["boards"]}
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    report_file = out / "report.json"
    report = json.load(open(report_file)) if report_file.exists() else {}

    prebuilt = {p["id"] for p in m["projects"] if p["firmware"] == "prebuilt"}
    todo = [
        (p["id"], b["board"])
        for p in m["projects"]
        if not a.only or p["id"] in a.only
        for b in p["builds"]
    ]
    print(f"{len(todo)} builds", flush=True)

    def save():
        report_file.write_text(json.dumps(report, indent=1, sort_keys=True))

    # Builds one at a time: arduino-cli shares its caches.
    for i, (project, board) in enumerate(todo, 1):
        key = f"{project}/{board}"
        entry = report.setdefault(key, {"project": project, "board": board, "chip": chip_of[board]})
        if project in prebuilt:
            entry["build"] = {"prebuilt": True}
            continue
        if not a.skip_build:
            entry["build"] = build(a.cli, project, board)
            print(f"[{i}/{len(todo)}] build {key}: {'ok' if entry['build']['ok'] else 'FAILED'} "
                  f"({entry['build']['seconds']} s)", flush=True)
            save()

    sims = [
        (k, e) for k, e in report.items()
        if (k.split("/")[0], k.split("/")[1]) in todo
        and e["chip"] in SIMULATED_CHIPS and e["board"] not in WORKBENCH_ONLY
        and not e.get("build", {}).get("prebuilt")
        and e.get("build", {}).get("ok", a.skip_build) is True
    ]
    with concurrent.futures.ThreadPoolExecutor(a.jobs) as pool:
        futs = {pool.submit(boot, a.cli, e["project"], e["board"], e["chip"], expect): k for k, e in sims}
        for f in concurrent.futures.as_completed(futs):
            k = futs[f]
            report[k]["boot"] = f.result()
            print(f"boot {k}: {report[k]['boot']['result']}", flush=True)
            save()
    save()
    return 0


if __name__ == "__main__":
    sys.exit(main())
