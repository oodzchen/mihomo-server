#!/usr/bin/python3
"""Owned test process for deterministic reload/restart failure injection."""
import http.server
import json
import pathlib
import re
import socketserver
import sys
import time
import urllib.parse

config = pathlib.Path(sys.argv[sys.argv.index("-f") + 1]).read_text()
if "-t" in sys.argv:
    sys.exit(0)
if "fixture-fail-start: true" in config or "DOMAIN,fail-start.test,DIRECT" in config:
    print("controlled candidate startup failure", flush=True)
    sys.exit(1)
mode = re.search(r"^mode:\s*(\w+)", config, re.MULTILINE).group(1)
proxy_api = "fixture-proxy-api: true" in config
empty = re.search(r"^fixture-empty-snapshots:\s*(\d+)", config, re.MULTILINE)
empty_snapshots = int(empty.group(1)) if empty else 0
queries = 0
choices = {"Main": "DIRECT", "Manual": "DIRECT"}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def respond(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        global queries
        if self.path == "/version":
            self.respond(200, {"version": "controlled-test-core", "meta": True})
        elif self.path == "/configs":
            self.respond(200, {"mode": mode})
        elif self.path == "/proxies" and proxy_api:
            if "fixture-proxy-delay: true" in config:
                time.sleep(30)
            queries += 1
            unloaded = queries <= empty_snapshots or "fixture-group-never-loads: true" in config
            proxies = {name: {"name": name, "type": "Selector", "all": ["DIRECT", "REJECT"], "now": choice} for name, choice in choices.items()}
            if unloaded:
                proxies["Main"]["all"] = []
                proxies["Main"]["now"] = None
            self.respond(200, {"proxies": proxies})
        else:
            self.respond(404, {"message": "unknown test route"})

    def do_PUT(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        group = urllib.parse.unquote(self.path.removeprefix("/proxies/"))
        if self.path.startswith("/proxies/") and proxy_api and group in choices:
            node = json.loads(body)["name"]
            if node == "REJECT" and "fixture-select-rejected: true" in config:
                self.respond(500, {"message": "controlled selection failure"})
            elif node not in ["DIRECT", "REJECT"]:
                self.respond(400, {"message": "unknown node"})
            else:
                if "fixture-ignore-selection: true" not in config:
                    choices[group] = node
                self.respond(200, {})
            return
        self.respond(500, {"message": "controlled reload failure"})


socket = sys.argv[sys.argv.index("-ext-ctl-unix") + 1]
with socketserver.UnixStreamServer(socket, Handler) as server:
    server.serve_forever()
