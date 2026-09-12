#!/usr/bin/env python3
"""Exercise the actual installer without network access or a Rust toolchain."""
import hashlib
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
VERSION = re.search(r'^version = "([^"\n]+)"', (ROOT / "herdr-plugin.toml").read_text(), re.M)[1]
BINARY = f'#!/bin/sh\necho "herdr-git-graph {VERSION}"\n'.encode()


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='hgg-install-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copytree(ROOT / 'scripts', self.root / 'scripts')
        shutil.copy(ROOT / 'herdr-plugin.toml', self.root)
        self.commands = self.root / 'commands'
        self.commands.mkdir()
        self.env = {**os.environ, 'PATH': str(self.commands), 'TEST_OS': 'Darwin',
                    'TEST_ARCH': 'arm64', 'TEST_ROOT': str(self.root)}
        self.env.pop('HERDR_GIT_GRAPH_BUILD_FROM_SOURCE', None)
        for command in ['sh', 'dirname', 'sed', 'mkdir', 'mktemp', 'rm', 'awk', 'shasum', 'chmod', 'mv']:
            (self.commands / command).symlink_to(shutil.which(command))
        self.script('uname', 'if [ "$1" = -s ]; then echo "$TEST_OS"; else echo "$TEST_ARCH"; fi')
        self.script('cargo', 'echo called > "$TEST_ROOT/cargo-called"; exit 99')
        (self.root / 'fixture').write_bytes(BINARY)
        self.digest = hashlib.sha256(BINARY).hexdigest()
        (self.root / 'checksum').write_text(self.digest + '  binary\n')
        self.script('curl', '''
[ "${TEST_DOWNLOAD_FAIL:-0}" = 0 ] || exit 22
while [ "$#" -gt 0 ]; do
    case "$1" in
        https://*) url=$1 ;;
        --output) shift; output=$1 ;;
    esac
    shift
done
printf '%s\\n' "$url" >> "$TEST_ROOT/urls"
case "$url" in
    *.sha256) input=checksum ;;
    *) input=fixture ;;
esac
while IFS= read -r line; do printf '%s\\n' "$line"; done < "$TEST_ROOT/$input" > "$output"
''')

    def script(self, name, body):
        path = self.commands / name
        path.write_text('#!/bin/sh\nset -eu\n' + body + '\n')
        path.chmod(0o755)

    def run_install(self, **env):
        result = subprocess.run(['/bin/sh', str(self.root / 'scripts/install.sh')],
                                cwd='/', env={**self.env, **env}, capture_output=True, text=True)
        self.assertFalse((self.root / 'cargo-called').exists(), result.stderr)
        self.assertEqual(list((self.root / 'bin').glob('.install.*')), [])
        return result

    def test_all_platforms_without_cargo(self):
        for system, arch, target in [
            ('Darwin', 'arm64', 'aarch64-apple-darwin'),
            ('Darwin', 'x86_64', 'x86_64-apple-darwin'),
            ('Linux', 'aarch64', 'aarch64-unknown-linux-gnu'),
            ('Linux', 'x86_64', 'x86_64-unknown-linux-gnu'),
        ]:
            with self.subTest(target=target):
                result = self.run_install(TEST_OS=system, TEST_ARCH=arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.root / 'bin/herdr-git-graph').read_bytes(), BINARY)
                self.assertIn(f'/v{VERSION}/herdr-git-graph-' + target, (self.root / 'urls').read_text())

    def test_sha256sum(self):
        self.script('sha256sum', 'exec shasum -a 256 "$@"')
        self.assertEqual(self.run_install().returncode, 0)

    def test_corruption_preserves_existing_binary(self):
        (self.root / 'bin').mkdir()
        installed = self.root / 'bin/herdr-git-graph'
        installed.write_text('previous executable')
        (self.root / 'checksum').write_text('0' * 64 + '\n')
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stderr)
        self.assertEqual(installed.read_text(), 'previous executable')

    def test_download_failure(self):
        result = self.run_install(TEST_DOWNLOAD_FAIL='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('download failed', result.stderr)
        self.assertFalse((self.root / 'bin/herdr-git-graph').exists())

    def test_unsupported_platform(self):
        result = self.run_install(TEST_ARCH='riscv64')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('No prebuilt binary', result.stderr)

    def test_source_build_missing_cargo_message(self):
        (self.commands / 'cargo').unlink()
        result = self.run_install(HERDR_GIT_GRAPH_BUILD_FROM_SOURCE='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Source builds require Rust and Cargo', result.stderr)


if __name__ == '__main__':
    unittest.main()
