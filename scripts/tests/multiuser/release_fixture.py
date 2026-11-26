#!/usr/bin/env python3
"""Serve rendered installer, checksummed releases and local core upgrade fixture."""
import gzip
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import platform
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

manifest = json.loads(Path('/work/bundle/resources/manifest.json').read_text())
core_version = manifest['core']['version']
core_prefix = 'mihomo-linux-arm64-' if platform.machine() in ('aarch64', 'arm64') else 'mihomo-linux-amd64-v2-'
core_asset = f'{core_prefix}{core_version}.gz'
core_dir = ROOT / 'MetaCubeX/mihomo/releases/download' / core_version
core_dir.mkdir(parents=True, exist_ok=True)
core_gz = gzip.compress(Path('/work/bundle/resources/core/verge-mihomo').read_bytes(), compresslevel=1)
(core_dir / core_asset).write_bytes(core_gz)
core_release_json = json.dumps({
    'tag_name': core_version,
    'draft': False,
    'prerelease': False,
    'assets': [{
        'name': core_asset,
        'state': 'uploaded',
        'size': len(core_gz),
        'digest': 'sha256:' + hashlib.sha256(core_gz).hexdigest(),
        'browser_download_url': f'http://127.0.0.1:18080/MetaCubeX/mihomo/releases/download/{core_version}/{core_asset}',
    }],
}).encode()


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
        if self.path == '/repos/MetaCubeX/mihomo/releases/latest':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(core_release_json)))
            self.end_headers()
            self.wfile.write(core_release_json)
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
