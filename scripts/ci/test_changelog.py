import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from changelog import release_notes


class ChangelogTests(unittest.TestCase):
    def setUp(self):
        environment = patch.dict(os.environ, {
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        })
        environment.start()
        self.addCleanup(environment.stop)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.git("init", "-q", "-b", "main")
        for key, value in (("user.name", "Release Test"), ("user.email", "test@example.org"),
                           ("commit.gpgsign", "false"), ("tag.gpgsign", "false"),
                           ("core.hooksPath", "/dev/null")):
            self.git("config", key, value)
        self.initial = self.commit("Initial")

    def git(self, *args):
        return subprocess.run(
            ("git", *args), cwd=self.root, check=True, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ).stdout.strip()

    def commit(self, message):
        self.git("commit", "--allow-empty", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def notes(self, tag, root=None):
        return release_notes(root or self.root, "https://codeberg.org", "owner/repo", tag)

    def test_component_history_contains_subjects_bodies_and_compare_links(self):
        self.git("tag", "-a", "client-v0.1.0", "-m", "Client 0.1.0")
        first = self.commit("Ajoute le menu\n\nRéglages indépendants.\n\n- Console\n- ROMs")
        self.git("tag", "server-v0.9.0")
        second = self.commit("Corrige la navigation")
        self.git("tag", "client-v0.2.0")
        self.commit("Travail postérieur exclu")
        notes = self.notes("client-v0.2.0")
        self.assertIn("## Modifications depuis client-v0.1.0", notes)
        self.assertIn("/compare/client-v0.1.0...client-v0.2.0", notes)
        self.assertIn(f"[{first[:8]}](https://codeberg.org/owner/repo/commit/{first})", notes)
        self.assertIn("      Réglages indépendants.", notes)
        self.assertIn("      - Console", notes)
        self.assertIn(second, notes)
        self.assertLess(notes.index(first), notes.index(second))
        self.assertNotIn(self.initial, notes)
        self.assertNotIn("Travail postérieur exclu", notes)
        self.assertNotIn("depuis server-v", notes)

    def test_server_uses_its_own_previous_tag_including_prereleases(self):
        self.git("tag", "server-v0.1.0")
        self.commit("Client seulement")
        self.git("tag", "client-v2.0.0")
        self.commit("Première prérelease")
        self.git("tag", "-a", "server-v0.2.0-rc.1", "-m", "RC")
        self.commit("Corrige la prérelease")
        self.git("tag", "server-v0.2.0")
        notes = self.notes("server-v0.2.0")
        self.assertIn("depuis server-v0.2.0-rc.1", notes)
        self.assertIn("Corrige la prérelease", notes)
        self.assertNotIn("Première prérelease", notes)
        self.assertNotIn("Client seulement", notes)

    def test_first_component_release_includes_full_history(self):
        self.git("tag", "v0.1.0")
        self.git("tag", "server-v0.1.0")
        self.commit("Première version client")
        self.git("tag", "client-v0.1.0")
        notes = self.notes("client-v0.1.0")
        self.assertIn("première release", notes)
        self.assertIn(self.initial, notes)
        self.assertIn("Première version client", notes)
        self.assertNotIn("/compare/", notes)

    def test_unreachable_and_invalid_tags_are_not_used_as_previous_releases(self):
        self.git("tag", "client-v0.1.0")
        self.git("switch", "-qc", "other")
        unrelated = self.commit("Branche non fusionnée")
        self.git("tag", "client-v9.0.0")
        self.git("switch", "-q", "main")
        self.commit("Changement principal")
        self.git("tag", "client-v-not-a-version")
        self.git("tag", "client-v4-invalid")
        self.commit("Release")
        self.git("tag", "client-v0.2.0")
        notes = self.notes("client-v0.2.0")
        self.assertIn("depuis client-v0.1.0", notes)
        self.assertNotIn(unrelated, notes)
        self.assertNotIn("client-v9.0.0", notes)

    def test_same_commit_tags_have_an_explicit_empty_changelog(self):
        self.git("tag", "client-v0.1.0")
        self.git("tag", "client-v0.1.1")
        notes = self.notes("client-v0.1.1")
        self.assertIn("depuis client-v0.1.0", notes)
        self.assertIn("Aucun nouveau commit", notes)

    def test_commit_markdown_is_preserved_as_text_not_interpreted(self):
        self.git("tag", "client-v0.1.0")
        self.commit("Corrige [liens](url) et <script>\n\n<script>alert(1)</script>\n```\n# Détails")
        self.git("tag", "client-v0.1.1")
        notes = self.notes("client-v0.1.1")
        self.assertIn(r"Corrige \[liens\](url) et \<script\>", notes)
        self.assertIn("      <script>alert(1)</script>", notes)
        self.assertIn("      ```", notes)
        self.assertIn("      # Détails", notes)

    def test_missing_or_invalid_tag_is_an_error_not_an_empty_release(self):
        for tag in ("client-v0.1.0", "--all", "v0.1.0"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                self.notes(tag)

    def test_shallow_checkout_is_rejected(self):
        self.git("tag", "client-v0.1.0")
        self.commit("Suivant")
        self.git("tag", "client-v0.1.1")
        clone = self.root / "clone"
        self.git("clone", "-q", "--depth", "1", self.root.as_uri(), str(clone))
        with self.assertRaisesRegex(ValueError, "fetch-depth: 0"):
            self.notes("client-v0.1.1", root=clone)


if __name__ == "__main__":
    unittest.main()
