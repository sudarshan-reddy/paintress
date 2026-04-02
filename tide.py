#!/usr/bin/env python3
"""Tide daemon: syncs e-ink displays to Thames tidal cycle.

On startup, fetches recent readings to determine the current tide state
and sends an image. Then sleeps until the next tidal turning point
(~6h 12m between high and low) and swaps the image. Repeats forever.
"""

import json
import os
import random
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timedelta, timezone

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HIGH_DIR = os.path.join(SCRIPT_DIR, "hightide")
LOW_DIR = os.path.join(SCRIPT_DIR, "lowtide")
PAINTRESS = os.path.join(SCRIPT_DIR, "target", "release", "paintress")

# Environment Agency flood monitoring API (free, no key needed)
# Station: Thames at Tower Pier (London) — readings every 15 min
READINGS_URL = (
    "https://environment.data.gov.uk/flood-monitoring/id/stations"
    "/E72639/readings?_sorted&_limit=100"
)

# Semi-diurnal tide: ~12h 25m full cycle, ~6h 12.5m between high and low
HALF_TIDE_MINUTES = 6 * 60 + 13


def log(msg):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] {msg}", file=sys.stderr, flush=True)


def fetch_readings():
    """Fetch the last ~25 hours of 15-min readings from Tower Pier."""
    req = urllib.request.Request(READINGS_URL, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        data = json.loads(resp.read())
    items = data.get("items", [])
    # Parse into (datetime, value) sorted oldest-first
    readings = []
    for item in items:
        v = item.get("value")
        if isinstance(v, list):
            v = v[0] if v else None
        if v is None:
            continue
        try:
            v = float(v)
        except (TypeError, ValueError):
            continue
        t = datetime.fromisoformat(item["dateTime"].replace("Z", "+00:00"))
        readings.append((t, v))
    readings.sort(key=lambda r: r[0])
    return readings


def find_turning_points(readings):
    """Find local maxima (high tides) and minima (low tides) in the readings.

    Returns list of (datetime, value, 'high'|'low') sorted by time.
    """
    if len(readings) < 3:
        return []
    turns = []
    for i in range(1, len(readings) - 1):
        prev_v = readings[i - 1][1]
        curr_t, curr_v = readings[i]
        next_v = readings[i + 1][1]
        if curr_v >= prev_v and curr_v >= next_v and curr_v != prev_v:
            turns.append((curr_t, curr_v, "high"))
        elif curr_v <= prev_v and curr_v <= next_v and curr_v != prev_v:
            turns.append((curr_t, curr_v, "low"))
    return turns


def get_tide_state_and_next_change():
    """Determine current tide state and when the next change occurs.

    Returns (state: 'high'|'low', next_change: datetime).
    """
    readings = fetch_readings()
    if len(readings) < 3:
        log("warning: not enough readings, defaulting to low")
        return "low", datetime.now(timezone.utc) + timedelta(minutes=HALF_TIDE_MINUTES)

    turns = find_turning_points(readings)
    now = datetime.now(timezone.utc)

    if turns:
        last_turn = turns[-1]
        last_turn_time, last_turn_value, last_turn_type = last_turn

        # Current state is opposite of the last turning point
        # (if last turn was high tide, we're now falling toward low)
        current_state = "low" if last_turn_type == "high" else "high"

        # Next change is ~6h 12m after the last turning point
        next_change = last_turn_time + timedelta(minutes=HALF_TIDE_MINUTES)

        # If the predicted next change is in the past, it's imminent — set it to now + 5min
        if next_change <= now:
            next_change = now + timedelta(minutes=5)

        log(f"last turn: {last_turn_type} tide at {last_turn_time.strftime('%H:%M')} ({last_turn_value:.2f}m)")
        log(f"current state: {current_state} tide")
        log(f"next change: ~{next_change.strftime('%H:%M')} UTC")
        return current_state, next_change
    else:
        # No turning points found — use raw trend
        latest = readings[-1][1]
        previous = readings[-2][1]
        state = "high" if latest >= previous else "low"
        next_change = now + timedelta(minutes=HALF_TIDE_MINUTES)
        log(f"no turning points found, using trend: {state} tide")
        return state, next_change


def pick_image(folder):
    """Pick a random image from the given folder."""
    exts = {".jpg", ".jpeg", ".png", ".bmp", ".webp"}
    images = [f for f in os.listdir(folder) if os.path.splitext(f)[1].lower() in exts]
    if not images:
        log(f"error: no images found in {folder}")
        sys.exit(1)
    return os.path.join(folder, random.choice(images))


def send_image(state, sleep_seconds=None):
    """Pick and send an image for the given tide state."""
    folder = HIGH_DIR if state == "high" else LOW_DIR
    image = pick_image(folder)
    log(f"sending {state} tide image: {os.path.basename(image)}")
    try:
        cmd = [PAINTRESS, "send", image]
        if sleep_seconds is not None:
            cmd.extend(["--sleep", str(sleep_seconds)])
        subprocess.run(cmd, check=True)
        log("send complete")
    except subprocess.CalledProcessError as e:
        log(f"send failed (exit {e.returncode}), will retry next cycle")


def main():
    log("tide daemon starting")

    while True:
        try:
            state, next_change = get_tide_state_and_next_change()
        except Exception as e:
            log(f"API error: {e}, retrying in 5 minutes")
            time.sleep(300)
            continue

        # Calculate sleep duration for displays (wake 2 min early for WiFi reconnect)
        now = datetime.now(timezone.utc)
        wait_seconds = max(0, (next_change - now).total_seconds())
        display_sleep = int(wait_seconds - 120)
        sleep_arg = display_sleep if display_sleep > 60 else None

        send_image(state, sleep_seconds=sleep_arg)

        # Sleep until the next tidal change
        now = datetime.now(timezone.utc)
        wait_seconds = max(0, (next_change - now).total_seconds())
        next_state = "low" if state == "high" else "high"
        log(f"sleeping {wait_seconds / 60:.0f} minutes until {next_state} tide")
        time.sleep(wait_seconds)


if __name__ == "__main__":
    main()
