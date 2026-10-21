import sys, json

def send(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()

def log(level, message):
    send({"type": "log", "level": level, "message": message})

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    kind = msg.get("type")

    if kind == "init":
        send({"type": "ready", "protocol": 1})

    elif kind == "query":
        who = msg["text"].strip() or "world"
        send({"type": "results", "id": msg["id"], "items": [{
            "key": "greet",
            "title": "Hello, " + who + "!",
            "subtitle": "Enter copies it, Right arrow for more",
            "score": 80,
            "copy": "Hello, " + who + "!",
            "data": {"who": who},
            "default": "copy",
            "actions": [
                {"id": "copy", "label": "Copy greeting", "do": {"copy": "Hello, " + who + "!"}},
                {"id": "docs", "label": "Open Python docs", "do": {"open": "https://docs.python.org/3/"}},
                {"id": "shout", "label": "Shout it", "stay": True},
            ],
        }]})

    elif kind == "action":
        if msg["id"] == "shout":
            who = msg.get("data", {}).get("who", "world")
            send({"type": "done", "req": msg["req"], "set_query": "hello " + who.upper() + "!",
                  "keep_open": True})
        else:
            send({"type": "done", "req": msg["req"]})

    elif kind == "shutdown":
        break
