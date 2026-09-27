#!/usr/bin/env python3
"""Hermetic concurrent/failure fixtures for the generated macOS install step."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / '.github/workflows/release.yml').read_text()
EXPRESSION = '${{ matrix.install_dist.run }}'


def step(name):
    block = WORKFLOW.split('      - name: ' + name + '\n', 1)[1].split('      - ', 1)[0]
    return block


class DistIsolation(unittest.TestCase):
    def test_scope_and_generation_contract(self):
        self.assertIn('allow-dirty = ["ci"]', (ROOT / 'dist-workspace.toml').read_text())
        self.assertEqual(WORKFLOW.count('      - name: Install dist (self-hosted macOS)'), 1)
        self.assertEqual(WORKFLOW.count('      - name: Install dist (hosted Linux)'), 1)
        self.assertIn("if: ${{ runner.os == 'macOS' }}", step('Install dist (self-hosted macOS)'))
        self.assertIn("if: ${{ runner.os != 'macOS' }}", step('Install dist (hosted Linux)'))
        self.assertIn('run: ' + EXPRESSION, step('Install dist (hosted Linux)'))
        self.assertIn('if: ${{ always() && runner.os == \'macOS\' }}', step('Verify persistent Cargo dist entries (macOS)'))
        self.assertLess(WORKFLOW.index('Install dist (self-hosted macOS)'), WORKFLOW.index('Build artifacts'))
        self.assertGreater(WORKFLOW.index('Verify persistent Cargo dist entries (macOS)'), WORKFLOW.index('Build artifacts'))
        self.assertIn('path: ~/.cargo/bin/dist', WORKFLOW)  # hosted plan cache unchanged
        self.assertEqual(WORKFLOW.count('      - name: Cache dist'), 1)
        self.assertIn('dist build ${{ needs.plan.outputs.tag-flag }}', WORKFLOW)

    def test_concurrent_success_and_failures(self):
        script = step('Install dist (self-hosted macOS)').split('        run: |\n', 1)[1]
        script = '\n'.join(line[10:] for line in script.splitlines())
        script = script.replace(EXPRESSION, 'curl --fake | sh')
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            fakebin = base / 'bin'
            fakebin.mkdir()
            curl = fakebin / 'curl'
            curl.write_text('''#!/bin/sh
if [ "${FAIL_INSTALL:-0}" = 1 ]; then exit 22; fi
if [ "${FALLBACK:-0}" = 1 ]; then
  echo 'mkdir -p "$HOME/.cargo/bin"; touch "$HOME/.cargo/bin/dist"'
  exit 0
fi
printf '%s\\n' 'mkdir -p "$CARGO_DIST_INSTALL_DIR/bin"' \\
  'printf "#!/bin/sh\\\\nexit 0\\\\n" > "$CARGO_DIST_INSTALL_DIR/bin/dist"' \\
  'chmod +x "$CARGO_DIST_INSTALL_DIR/bin/dist"'
''')
            curl.chmod(0o755)
            home = base / 'home'
            (home / '.cargo/bin').mkdir(parents=True)
            existing = home / '.cargo/bin/cargo-dist'
            existing.write_text('do not touch')
            env = dict(os.environ, HOME=str(home), RUNNER_TEMP=str(base),
                       PATH=f'{fakebin}:{os.environ["PATH"]}')
            outputs = [base / f'path-{i}' for i in range(4)]
            for output in outputs:
                output.touch()
            procs = [subprocess.Popen(['bash', '-c', script],
                     env=dict(env, GITHUB_PATH=str(output), FAIL_INSTALL='1' if i == 2 else '0',
                              FALLBACK='1' if i == 3 else '0'), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for i, output in enumerate(outputs)]
            results = [p.communicate() for p in procs]
            self.assertEqual([p.returncode for p in procs], [0, 0, 22, 1], results)
            roots = [o.read_text().strip() for o in outputs]
            self.assertEqual(roots[2:], ['', ''])
            self.assertEqual(len(set(roots[:2])), 2)
            for root in roots[:2]:
                self.assertTrue(root.startswith(str(base / 'taskfleet-dist.')))
                self.assertTrue((Path(root) / 'dist').is_file())
                resolved = subprocess.check_output(['bash', '-c', 'command -v dist'],
                         env=dict(env, PATH=f'{root}:{env["PATH"]}'), text=True).strip()
                self.assertEqual(resolved, str(Path(root) / 'dist'))
            self.assertEqual(existing.read_text(), 'do not touch')
            self.assertTrue((home / '.cargo/bin/dist').exists())  # fallback caught, not erased
            snapshot = base / 'snapshot.json'
            checker = ROOT / 'scripts/check-macos-dist-bin.py'
            snapshot.touch()
            subprocess.run([sys.executable, checker, 'snapshot', snapshot], env=env, check=True)
            subprocess.run([sys.executable, checker, 'verify', snapshot], env=env, check=True)
            (home / '.cargo/bin/dist').write_text('changed')
            changed = subprocess.run([sys.executable, checker, 'verify', snapshot], env=env,
                                     capture_output=True, text=True)
            self.assertNotEqual(changed.returncode, 0)
            self.assertIn('entries changed', changed.stderr)
            self.assertEqual(existing.read_text(), 'do not touch')


if __name__ == '__main__':
    unittest.main()
