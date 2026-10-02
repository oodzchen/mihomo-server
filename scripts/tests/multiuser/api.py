#!/usr/bin/env python3
"""Call one authenticated management command: api.py PORT TOKEN_FILE COMMAND [JSON_FIELDS | @FILE]."""
import json
import sys
import urllib.request

port, token_file, command = sys.argv[1:4]
raw = sys.argv[4] if len(sys.argv) > 4 else "{}"
fields = json.loads(open(raw[1:]).read() if raw.startswith("@") else raw)
token = open(token_file).read().strip()
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/api/commands",
    data=json.dumps({"command": command, **fields}).encode(),
    headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
)
# The management API is local; never route it through an inherited proxy.
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
try:
    with opener.open(request, timeout=120) as response:
        print(response.read().decode())
except urllib.error.HTTPError as error:
    print(error.read().decode(), file=sys.stderr)
    sys.exit(1)
