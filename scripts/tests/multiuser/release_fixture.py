#!/usr/bin/env python3
"""Serve rendered installer and checksummed releases for the real download path."""
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tarfile

ROOT = Path('/work/releases')
ROOT.mkdir(exist_ok=True)
(ROOT / 'latest-tag').write_text('vfixture1')
script = Path('/work/install.sh').read_text().replace('local REPO="__REPO_SLUG__"', 'local REPO="test/repo"')
for tag in ['vfixture1', 'vfixture2', 'vfixture3']:
    name = f'mihomo-server-{tag}-x86_64-unknown-linux-gnu'
    destination = ROOT / 'test/repo/releases/download' / tag
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / (name + '.tar.gz')
    with tarfile.open(archive, 'w:gz', compresslevel=1) as tar:
        tar.add('/work/bundle', arcname=name)
    (destination / (name + '.tar.gz.sha256')).write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')


class Releases(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def do_GET(self):
        if self.path == '/test/repo/releases/latest':
            self.send_response(302)
            self.send_header('Location', '/test/repo/releases/tag/' + (ROOT / 'latest-tag').read_text().strip())
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        if self.path == '/test/repo/releases/latest/download/install.sh':
            body = script.encode()
        else:
            return super().do_GET()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


ThreadingHTTPServer(('127.0.0.1', 18080), Releases).serve_forever()
