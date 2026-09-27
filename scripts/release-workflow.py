#!/usr/bin/env python3
"""Check/write the pinned cargo-dist workflow plus the self-hosted macOS override.

Generate in an external disposable workspace: native `dist generate --check`
ignores CI with allow-dirty = ["ci"], so it cannot detect drift in release.yml.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = Path('.github/workflows/release.yml')
ORIGINAL = """      - name: Install dist
        run: ${{ matrix.install_dist.run }}
      # Get the dist-manifest"""
REPLACEMENT = """      # Taskfleet override: only the self-hosted macOS row needs a private dist.
      # The hosted Linux rows retain the upstream matrix installer and cache.
      - name: Snapshot persistent Cargo dist entries (macOS)
        if: ${{ runner.os == 'macOS' }}
        id: dist-baseline
        shell: bash
        run: |
          set -euo pipefail
          : "${RUNNER_TEMP:?RUNNER_TEMP must be set}"
          snapshot="$(mktemp "$RUNNER_TEMP/taskfleet-dist-baseline.XXXXXXXX")"
          python3 scripts/check-macos-dist-bin.py snapshot "$snapshot"
          echo "snapshot=$snapshot" >> "$GITHUB_OUTPUT"
      - name: Install dist (self-hosted macOS)
        if: ${{ runner.os == 'macOS' }}
        shell: bash
        run: |
          set -euo pipefail
          : "${RUNNER_TEMP:?RUNNER_TEMP must be set for isolated dist install}"
          export CARGO_DIST_INSTALL_DIR="$(mktemp -d "$RUNNER_TEMP/taskfleet-dist.XXXXXXXX")"
          export CARGO_DIST_NO_MODIFY_PATH=1
          ${{ matrix.install_dist.run }}
          # Never use an older PATH dist or silently accept an installer fallback.
          export PATH="$CARGO_DIST_INSTALL_DIR/bin:$PATH"
          test "$(command -v dist)" = "$CARGO_DIST_INSTALL_DIR/bin/dist"
          echo "$CARGO_DIST_INSTALL_DIR/bin" >> "$GITHUB_PATH"
      - name: Install dist (hosted Linux)
        if: ${{ runner.os != 'macOS' }}
        run: ${{ matrix.install_dist.run }}
      # Get the dist-manifest"""
END_ORIGINAL = """            ${{ env.BUILD_MANIFEST_NAME }}

  # Build and package all the platform-agnostic(ish) things"""
END_REPLACEMENT = """            ${{ env.BUILD_MANIFEST_NAME }}
      # Run even after a failed install/build; never hide persistent-bin drift.
      - name: Verify persistent Cargo dist entries (macOS)
        if: ${{ always() && runner.os == 'macOS' }}
        shell: bash
        run: |
          set -euo pipefail
          python3 scripts/check-macos-dist-bin.py verify '${{ steps.dist-baseline.outputs.snapshot }}'

  # Build and package all the platform-agnostic(ish) things"""


def generate(dist):
    # Cargo metadata walks up to the workspace root; generate outside this tree.
    with tempfile.TemporaryDirectory(prefix='taskfleet-dist-generate-') as tmp:
        with subprocess.Popen(['git', 'archive', 'HEAD'], cwd=ROOT, stdout=subprocess.PIPE) as git:
            subprocess.run(['tar', '-xf', '-', '-C', tmp], stdin=git.stdout, check=True)
            git.stdout.close()
            if git.wait() != 0:
                raise RuntimeError('git archive failed')
        for name in ('Cargo.toml', 'Cargo.lock'):
            shutil.copy2(ROOT / name, Path(tmp) / name)
        config = (ROOT / 'dist-workspace.toml').read_text()
        marker = 'allow-dirty = ["ci"]\n'
        if config.count(marker) != 1:
            raise RuntimeError('expected exactly one approved cargo-dist allow-dirty override')
        (Path(tmp) / 'dist-workspace.toml').write_text(config.replace(marker, ''))
        subprocess.run([dist, 'generate', '--mode', 'ci'], cwd=tmp, check=True)
        generated = (Path(tmp) / WORKFLOW).read_text()
        if generated.count(ORIGINAL) != 1 or generated.count(END_ORIGINAL) != 1:
            raise RuntimeError('cargo-dist local build template changed; review override')
        return generated.replace(ORIGINAL, REPLACEMENT).replace(END_ORIGINAL, END_REPLACEMENT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    operation = parser.add_mutually_exclusive_group(required=True)
    operation.add_argument('--check', action='store_true')
    operation.add_argument('--write', action='store_true')
    parser.add_argument('--dist', default='dist', help='path to pinned cargo-dist 0.33.0')
    args = parser.parse_args()
    dist = shutil.which(args.dist)
    if not dist:
        parser.error(f'dist binary not found: {args.dist}')
    if subprocess.check_output([dist, '--version'], text=True).strip() != 'cargo-dist 0.33.0':
        parser.error('requires cargo-dist 0.33.0')
    expected = generate(str(Path(dist).resolve()))
    path = ROOT / WORKFLOW
    if args.write:
        path.write_text(expected)
        print(f'generated {WORKFLOW}')
    elif path.read_text() != expected:
        raise SystemExit(f'{WORKFLOW} differs from generated workflow + macOS override; run --write')
    else:
        print(f'{WORKFLOW}: generated workflow + macOS override match')


if __name__ == '__main__':
    main()
