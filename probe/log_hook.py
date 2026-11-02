import json, os, sys, time
raw = sys.stdin.read()
d = json.loads(raw) if raw.strip() else {}
ev = d.get("hook_event_name", "unknown")
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs", f"{time.time_ns()}_{ev}.json")
with open(out, "w") as f:
    json.dump(d, f, ensure_ascii=False, indent=2)
