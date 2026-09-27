#!/usr/bin/env python3
"""Record/verify the two persistent Cargo dist entrypoints without touching them.

The before/after check is diagnostic; the private RUNNER_TEMP installer and
command -v assertion are the actual prevention. Runs with `always()` on macOS.
"""
import hashlib
import json
import os
from pathlib import Path
import stat
import sys


def identity(path):
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    row = {'mode': info.st_mode, 'dev': info.st_dev, 'ino': info.st_ino,
           'size': info.st_size, 'mtime_ns': info.st_mtime_ns}
    if stat.S_ISREG(info.st_mode):
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        row['sha256'] = digest.hexdigest()
    elif stat.S_ISLNK(info.st_mode):
        row['link'] = os.readlink(path)
    return row


def inventory():
    root = Path.home() / '.cargo' / 'bin'
    return {name: identity(root / name) for name in ('dist', 'cargo-dist')}


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ('snapshot', 'verify') or not sys.argv[2]:
        sys.exit('usage: check-macos-dist-bin.py snapshot|verify <snapshot-path>')
    action, filename = sys.argv[1:]
    path = Path(filename)
    if action == 'snapshot':
        # mktemp has already created this file inside RUNNER_TEMP with mode 0600.
        if not path.is_file():
            sys.exit('baseline snapshot must be a pre-created file')
        path.write_text(json.dumps(inventory(), sort_keys=True))
        print('recorded persistent Cargo dist/cargo-dist baseline')
    else:
        try:
            before = json.loads(path.read_text())
        except (OSError, ValueError) as exc:
            sys.exit(f'cannot verify persistent Cargo dist baseline: {exc}')
        after = inventory()
        if before != after:
            sys.exit('persistent Cargo dist/cargo-dist entries changed during macOS release job')
        print('persistent Cargo dist/cargo-dist entries unchanged')


if __name__ == '__main__':
    main()
