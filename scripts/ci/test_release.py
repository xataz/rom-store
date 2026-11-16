import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from package import package, validate_tag
from publish import publish


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for crate in ("client", "server"):
            self.write(f"{crate}/Cargo.toml", b'[package]\nversion = "0.1.0"\n')
            # En-tête ELF64 little-endian avec e_machine = EM_X86_64.
            elf = b"\x7fELF\x02\x01" + bytes(12) + b"\x3e\x00" + bytes(44)
            path = self.write(f"{crate}/target/release/romstore-{crate}", elf)
            path.chmod(0o755)
        for name in ("README.md", "docs/ci.md", "docs/server.md", "client/config.example.yaml"):
            self.write(name, b"Demo\n")

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return path

    def test_tag_matches_both_crate_versions_and_rejects_paths(self):
        self.assertEqual(validate_tag(self.root, "v0.1.0"), "0.1.0")
        for tag in ("../v0.1.0", "0.1.0", "v0.2.0", "v0.1.0;echo bad"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(self.root, tag)
        self.write("server/Cargo.toml", b'[package]\nversion = "0.2.0"\n')
        with self.assertRaises(ValueError):
            validate_tag(self.root, "v0.1.0")

    def test_archives_have_expected_files_modes_and_checksums(self):
        package(self.root, "v0.1.0")
        output = self.root / "dist/v0.1.0"
        lines = (output / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(lines), 2)
        for line in lines:
            digest, filename = line.split("  ")
            archive = output / filename
            self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), digest)
            with tarfile.open(archive) as tar:
                prefix = filename.removesuffix(".tar.gz")
                crate = "client" if "client" in filename else "server"
                binary = tar.getmember(f"{prefix}/romstore-{crate}")
                self.assertTrue(binary.mode & 0o111)
                self.assertIn(f"{prefix}/docs/ci.md", tar.getnames())
                self.assertTrue(all(name.startswith(prefix + "/") for name in tar.getnames()))

    def test_wrong_architecture_is_rejected_before_writing_archives(self):
        self.write("server/target/release/romstore-server", b"not an x86_64 ELF")
        with self.assertRaises(ValueError):
            package(self.root, "v0.1.0")
        self.assertFalse((self.root / "dist").exists())

    def do_publish(self):
        publish(self.root, "https://codeberg.org", "owner/repo", "v0.1.0", "abc123", "test-token")

    def test_release_is_published_only_after_all_assets_are_uploaded(self):
        package(self.root, "v0.1.0")
        responses = [HTTPError("url", 404, "not found", {}, None)]
        responses += [io.BytesIO(json.dumps(value).encode()) for value in (
            {"id": 42}, {"id": 1}, {"id": 2}, {"id": 3}, {"draft": False},
        )]
        with patch("publish.urlopen", side_effect=responses) as api:
            self.do_publish()
        requests = [call.args[0] for call in api.call_args_list]
        self.assertEqual([req.method for req in requests], ["GET", "POST", "POST", "POST", "POST", "PATCH"])
        self.assertTrue(json.loads(requests[1].data)["draft"])
        self.assertFalse(json.loads(requests[-1].data)["draft"])
        self.assertIn(b'name="attachment"', requests[2].data)
        self.assertIn(b"SHA256SUMS", requests[4].data)

    def test_upload_failure_leaves_release_as_draft(self):
        package(self.root, "v0.1.0")
        responses = [HTTPError("url", 404, "not found", {}, None),
                     io.BytesIO(b'{"id": 42}'), HTTPError("url", 500, "upload failed", {}, None)]
        with patch("publish.urlopen", side_effect=responses) as api:
            with self.assertRaises(HTTPError):
                self.do_publish()
        self.assertNotIn("PATCH", [call.args[0].method for call in api.call_args_list])

    def test_existing_release_is_not_modified(self):
        package(self.root, "v0.1.0")
        with patch("publish.urlopen", return_value=io.BytesIO(b'{"id": 42}')) as api:
            with self.assertRaisesRegex(ValueError, "existe déjà"):
                self.do_publish()
        self.assertEqual(api.call_count, 1)

    def test_missing_asset_prevents_any_api_call(self):
        with patch("publish.urlopen") as api:
            with self.assertRaisesRegex(ValueError, "absent"):
                self.do_publish()
        api.assert_not_called()


if __name__ == "__main__":
    unittest.main()
