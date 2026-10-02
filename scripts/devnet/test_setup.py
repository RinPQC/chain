#!/usr/bin/env python3
"""Exercise real key/config initialization without requiring a Docker daemon."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("container.py").resolve()
BINARY = Path(os.environ.get("RINPQC_NODE_BIN", "target/debug/rinpqc-node")).resolve()


class SetupTests(unittest.TestCase):
    def test_initialization_reuse_partial_state_and_unused_guard(self):
        with tempfile.TemporaryDirectory() as root:
            directories = [Path(root) / f"node{i}" for i in range(4)]
            for directory in directories:
                directory.mkdir(mode=0o700)
            env = dict(os.environ, DEVNET_ROOT=root, DEVNET_PREFIX="172.30.91", RINPQC_NODE_BIN=str(BINARY))
            def command(name):
                return subprocess.run(["python3", str(SCRIPT), name], env=env, capture_output=True, text=True, timeout=30)
            first = command("initialize")
            self.assertEqual(first.returncode, 0, first.stderr)
            metadata = [json.loads((directory / "devnet.json").read_text()) for directory in directories]
            self.assertTrue(all(data == metadata[0] for data in metadata))
            self.assertEqual(len({(directory / "validator.key").read_bytes() for directory in directories}), 4)
            original = {str(path): path.read_bytes() for directory in directories for path in directory.iterdir() if path.is_file()}
            self.assertEqual(command("initialize").returncode, 0)
            self.assertEqual(original, {str(path): path.read_bytes() for directory in directories for path in directory.iterdir() if path.is_file()})
            self.assertEqual(command("assert-unused").returncode, 0)
            (directories[0] / "ever_started").touch(mode=0o600)
            self.assertNotEqual(command("assert-unused").returncode, 0)
            (directories[2] / "initialized").unlink()
            result = command("initialize")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Partial or foreign", result.stderr)
            self.assertEqual((directories[0] / "validator.key").read_bytes(), original[str(directories[0] / "validator.key")])


if __name__ == "__main__":
    unittest.main()
