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

    def prepare(self, component="client", release_type="minor", runner=None, cargo="cargo"):
        output = io.StringIO()
        with patch("prepare_release.run", side_effect=runner or self.fake_run):
            with contextlib.redirect_stdout(output):
                prepare_release.prepare(self.root, component, release_type, cargo)
        return output.getvalue()

    def test_prepares_only_client_and_creates_annotated_tag_without_push(self):
        output = self.prepare()
        self.assertIn("Release client (minor) : 0.1.0 → 0.2.0", output)
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

    def test_prepares_server_major_and_respects_cargo_override(self):
        self.prepare("server", "major", cargo="my-cargo +stable")
        self.assertEqual(self.git("tag", "--list"), "server-v1.0.0")
        self.assertEqual(self.cargo_calls[0][:3], ("my-cargo", "+stable", "metadata"))
        self.assertIn('version = "1.0.0"', (self.root / "server/Cargo.lock").read_text())
        self.assertEqual(self.git("show", "HEAD:client/Cargo.toml"),
                         self.git("show", f"{self.head}:client/Cargo.toml"))

    def test_prepares_client_fix(self):
        self.prepare(release_type="fix")
        self.assertEqual(self.git("tag", "--list"), "client-v0.1.1")
        for filename in ("Cargo.toml", "Cargo.lock"):
            self.assertIn('version = "0.1.1"', (self.root / "client" / filename).read_text())
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_rejects_invalid_types_before_modifying_anything(self):
        for release_type in ("", "patch", "MINOR", "0.2.0", "fix;echo bad"):
            with self.subTest(release_type=release_type), self.assertRaisesRegex(ValueError, "TYPE"):
                self.prepare(release_type=release_type)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.cargo_calls, [])

    def test_rejects_existing_tag(self):
        self.git("tag", "client-v0.2.0")
        with self.assertRaisesRegex(ValueError, "existe déjà"):
            self.prepare()
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.cargo_calls, [])

    def test_rejects_current_prerelease_before_modifying_anything(self):
        manifest = self.root / "client/Cargo.toml"
        manifest.write_text(manifest.read_text().replace('"0.1.0"', '"0.2.0-rc.1"'))
        self.git("add", "client/Cargo.toml")
        self.git("commit", "-qm", "Prerelease")
        head = self.git("rev-parse", "HEAD")
        with self.assertRaisesRegex(ValueError, "stable"):
            self.prepare()
        self.assertEqual(self.git("rev-parse", "HEAD"), head)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("tag", "--list"), "")
        self.assertEqual(self.cargo_calls, [])

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


class NextVersionTests(unittest.TestCase):
    def test_increments_selected_level_and_resets_lower_levels(self):
        cases = (
            ("1.2.3", "fix", "1.2.4"),
            ("1.2.3", "minor", "1.3.0"),
            ("1.2.3", "major", "2.0.0"),
            ("0.0.0", "fix", "0.0.1"),
            ("0.1.9", "fix", "0.1.10"),
            ("0.9.9", "minor", "0.10.0"),
            ("9.9.9", "major", "10.0.0"),
        )
        for current, release_type, expected in cases:
            with self.subTest(current=current, release_type=release_type):
                self.assertEqual(prepare_release.next_version(current, release_type), expected)

    def test_rejects_nonstable_or_invalid_current_versions(self):
        for current in ("", "v0.2.0", "0.2", "01.2.0", "0.2.0-rc.1", "0.2.0+build", 1):
            with self.subTest(current=current), self.assertRaisesRegex(ValueError, "stable"):
                prepare_release.next_version(current, "fix")

    def test_rejects_invalid_release_type(self):
        with self.assertRaisesRegex(ValueError, "TYPE"):
            prepare_release.next_version("1.2.3", "patch")


if __name__ == "__main__":
    unittest.main()
