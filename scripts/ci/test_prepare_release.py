import contextlib
import io
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import prepare_release


class PrepareReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.git("init", "-q", "-b", "main")
        for key, value in (("user.name", "Release Test"), ("user.email", "test@example.org"),
                           ("commit.gpgsign", "false"), ("tag.gpgsign", "false"),
                           ("core.hooksPath", "/dev/null")):
            self.git("config", key, value)
        for component in ("client", "server"):
            directory = self.root / component
            directory.mkdir()
            (directory / "Cargo.toml").write_text(
                f'[package]\nname = "romstore-{component}"\nversion = "0.1.0" # Version\n'
                '\n[dependencies]\nexample = { version = "1" }\n'
            )
            (directory / "Cargo.lock").write_text(
                f'[[package]]\nname = "romstore-{component}"\nversion = "0.1.0"\n'
            )
        self.git("add", ".")
        self.git("commit", "-qm", "Initial")
        self.head = self.git("rev-parse", "HEAD")
        self.cargo_calls = []
        self.real_run = prepare_release.run

    def git(self, *args):
        return subprocess.run(
            ("git", *args), cwd=self.root, check=True, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ).stdout.strip()

    def fake_run(self, root, *args, **kwargs):
        if args[0] == "git":
            return self.real_run(root, *args, **kwargs)
        self.cargo_calls.append(args)
        if "metadata" in args:
            manifest = root / args[args.index("--manifest-path") + 1]
            version = tomllib.loads(manifest.read_text())["package"]["version"]
            lock = manifest.with_name("Cargo.lock")
            lock.write_text(lock.read_text().replace('version = "0.1.0"', f'version = "{version}"'))

    def prepare(self, component="client", version="0.2.0", runner=None, cargo="cargo"):
        output = io.StringIO()
        with patch("prepare_release.run", side_effect=runner or self.fake_run):
            with contextlib.redirect_stdout(output):
                prepare_release.prepare(self.root, component, version, cargo)
        return output.getvalue()

    def test_prepares_only_client_and_creates_annotated_tag_without_push(self):
        output = self.prepare()
        self.assertIn("git push origin main", output)
        self.assertIn("git push origin client-v0.2.0", output)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("cat-file", "-t", "client-v0.2.0"), "tag")
        self.assertEqual(self.git("rev-parse", "client-v0.2.0^{}"), self.git("rev-parse", "HEAD"))
        self.assertEqual(self.git("diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"),
                         "client/Cargo.lock\nclient/Cargo.toml")
        self.assertEqual(self.git("show", "HEAD:server/Cargo.toml"),
                         self.git("show", f"{self.head}:server/Cargo.toml"))
        self.assertEqual([call[1] for call in self.cargo_calls],
                         ["metadata", "fmt", "clippy", "test"])
        self.assertNotIn("--locked", self.cargo_calls[0])
        self.assertIn("--locked", self.cargo_calls[2])
        self.assertIn("--locked", self.cargo_calls[3])
        self.assertIn('version = "0.2.0" # Version', (self.root / "client/Cargo.toml").read_text())
        self.assertIn('example = { version = "1" }', (self.root / "client/Cargo.toml").read_text())

    def test_prepares_server_prerelease_and_respects_cargo_override(self):
        self.prepare("server", "0.2.0-rc.1", cargo="my-cargo +stable")
        self.assertEqual(self.git("tag", "--list"), "server-v0.2.0-rc.1")
        self.assertEqual(self.cargo_calls[0][:3], ("my-cargo", "+stable", "metadata"))
        self.assertIn('version = "0.2.0-rc.1"', (self.root / "server/Cargo.lock").read_text())

    def test_rejects_invalid_versions_before_modifying_anything(self):
        for version in ("", "v0.2.0", "0.2", "01.2.0", "0.2.0-01", "0.2.0-rc..1",
                        "0.2.0+build", "0.2.0;echo bad"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                self.prepare(version=version)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.cargo_calls, [])

    def test_rejects_existing_tag(self):
        self.git("tag", "client-v0.2.0")
        with self.assertRaisesRegex(ValueError, "existe déjà"):
            self.prepare()
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.cargo_calls, [])

    def test_rejects_same_version(self):
        with self.assertRaisesRegex(ValueError, "déjà en version"):
            self.prepare(version="0.1.0")
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_rejects_dirty_tracked_staged_and_untracked_files(self):
        manifest = self.root / "client/Cargo.toml"
        manifest.write_text(manifest.read_text() + "# Work in progress\n")
        with self.assertRaisesRegex(ValueError, "propre"):
            self.prepare()
        self.git("add", "client/Cargo.toml")
        with self.assertRaisesRegex(ValueError, "propre"):
            self.prepare()
        self.git("commit", "-qm", "User work")
        (self.root / "notes.txt").write_text("Do not delete")
        with self.assertRaisesRegex(ValueError, "propre"):
            self.prepare()
        self.assertEqual((self.root / "notes.txt").read_text(), "Do not delete")
        self.assertEqual(self.cargo_calls, [])

    def test_rejects_detached_head_before_modifying_files(self):
        self.git("switch", "--detach")
        with self.assertRaises(subprocess.CalledProcessError):
            self.prepare()
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.cargo_calls, [])

    def assert_check_failure(self, stage):
        def fail(root, *args, **kwargs):
            if args[:2] == ("cargo", stage):
                raise subprocess.CalledProcessError(1, args)
            return self.fake_run(root, *args, **kwargs)

        with self.assertRaises(subprocess.CalledProcessError):
            self.prepare(runner=fail)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.head)
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertIn("client/Cargo.toml", self.git("status", "--porcelain"))
        self.assertIn('version = "0.2.0"', (self.root / "client/Cargo.toml").read_text())

    def test_metadata_failure_does_not_commit_or_tag(self):
        self.assert_check_failure("metadata")

    def test_format_failure_does_not_commit_or_tag(self):
        self.assert_check_failure("fmt")

    def test_clippy_failure_does_not_commit_or_tag(self):
        self.assert_check_failure("clippy")

    def test_test_failure_does_not_commit_or_tag(self):
        self.assert_check_failure("test")

    def test_unrelated_changes_during_tests_prevent_commit_and_tag(self):
        def change(root, *args, **kwargs):
            result = self.fake_run(root, *args, **kwargs)
            if args[:2] == ("cargo", "test"):
                (root / "unexpected.txt").write_text("Test output")
            return result

        with self.assertRaisesRegex(ValueError, "autres que"):
            self.prepare(runner=change)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.head)
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertTrue((self.root / "unexpected.txt").exists())


if __name__ == "__main__":
    unittest.main()
