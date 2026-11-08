"""Focus Timer (Pomodoro) module for ByteForge: a countdown in the status bar."""

import json
import shutil
import subprocess
import sys
import threading
import time

FOCUS, SHORT_BREAK, LONG_BREAK = 25 * 60, 5 * 60, 15 * 60

lock = threading.Lock()
state_lock = threading.RLock()
state = {"phase": "idle", "remaining": FOCUS, "running": False, "done": 0}


def send(message):
    with lock:
        print(json.dumps(message), flush=True)


def notify(text):
    send({"type": "log", "text": f"Focus Timer: {text}"})
    if shutil.which("notify-send"):
        subprocess.Popen(["notify-send", "--app-name=ByteForge", "Focus Timer", text], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def render():
    phase, remaining, running = state["phase"], state["remaining"], state["running"]
    minutes, seconds = divmod(max(remaining, 0), 60)
    if phase == "idle":
        text, tip = "◷ Focus", "Start a 25-minute focus session"
    else:
        icon = "◉" if phase == "focus" else "☕"
        text = f"{icon} {minutes:02d}:{seconds:02d}" + ("" if running else " ⏸")
        tip = f"{'Focus' if phase == 'focus' else 'Break'} · {state['done']} sessions done today. Click to {'pause' if running else 'resume'}."
    send({"type": "status", "id": "focus.timer", "text": text, "tooltip": tip, "command": "focus.toggle", "alignment": "right"})


def next_phase():
    if state["phase"] == "focus":
        state["done"] += 1
        long = state["done"] % 4 == 0
        state["phase"], state["remaining"] = "break", LONG_BREAK if long else SHORT_BREAK
        notify(f"Session {state['done']} done. Take a {'15' if long else '5'}-minute break.")
    else:
        state["phase"], state["remaining"] = "focus", FOCUS
        notify("Break's over. Back to focus.")


def tick():
    while True:
        time.sleep(1)
        with state_lock:
            if state["running"]:
                state["remaining"] -= 1
                if state["remaining"] <= 0:
                    next_phase()
                render()


def toggle():
    if state["phase"] == "idle":
        state.update(phase="focus", remaining=FOCUS, running=True)
    else:
        state["running"] = not state["running"]
    render()


def reset():
    state.update(phase="idle", remaining=FOCUS, running=False)
    render()


def skip():
    if state["phase"] != "idle":
        next_phase()
        state["running"] = True
    render()


COMMANDS = {
    "focus.toggle": ("Start / Pause", toggle),
    "focus.skip": ("Skip to Next Phase", skip),
    "focus.reset": ("Stop and Reset", reset),
}
for command, (title, _) in COMMANDS.items():
    send({"type": "registerCommand", "id": command, "title": title, "category": "Focus Timer"})
render()
threading.Thread(target=tick, daemon=True).start()

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("type") == "executeCommand" and message.get("id") in COMMANDS:
        with state_lock:
            COMMANDS[message["id"]][1]()
