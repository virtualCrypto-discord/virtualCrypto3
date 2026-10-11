import argparse
import copy
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "fly_release", Path(__file__).resolve().parents[1] / "fly-release.py"
)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

REPOSITORY = "virtualCrypto-discord/virtualCrypto3"
SOURCE = "a" * 40
CANDIDATE = "registry.fly.io/virtualcrypto-staging@sha256:" + "b" * 64


def machine(digest="c" * 64):
    return {
        "id": "123abc", "version": "machine-version-1", "instance_id": "instance-1",
        "state": "started", "region": "lax", "checks": [],
        "image_ref": {"registry": "registry.fly.io", "repository": "virtualcrypto-prod",
                      "digest": "sha256:" + digest},
        "config": {"image": "registry.fly.io/virtualcrypto-prod:old",
                   "init": {"cmd": ["/bin/sh", "-c", "exec /app/bin/server"]},
                   "env": {"FLY_PROCESS_GROUP": "app"}, "services": [],
                   "guest": {"memory_mb": 512}},
    }


def secrets():
    return [{"name": name, "digest": "old", "status": "Deployed"}
            for name in sorted(release.REQUIRED_SECRETS)]


class ArtifactTrust(unittest.TestCase):
    def setUp(self):
        self.run = {
            "id": 123, "run_attempt": 2, "head_sha": SOURCE, "head_branch": "main",
            "head_repository": {"full_name": REPOSITORY}, "status": "completed",
            "event": "push", "path": ".github/workflows/ci.yml", "conclusion": "success",
        }
        self.record = {
            "schema": 1, "repository": REPOSITORY, "run_id": "123", "run_attempt": "2",
            "workflow_sha": SOURCE, "source_sha": SOURCE, "image": CANDIDATE,
            "operation": "staging", "verified": True,
        }

    def validate(self, kind="staging"):
        release.provenance(self.record, self.run, kind, REPOSITORY)

    def test_successful_main_ci_artifact_is_accepted(self):
        self.validate()

    def test_pr_fork_failure_old_attempt_and_changed_image_are_rejected(self):
        cases = [
            ("run", "event", "pull_request"),
            ("run", "head_branch", "feature/example"),
            ("run", "head_repository", {"full_name": "attacker/fork"}),
            ("run", "conclusion", "failure"),
            ("run", "path", ".github/workflows/other.yml"),
            ("record", "verified", False),
            ("record", "run_attempt", "1"),
            ("record", "source_sha", "d" * 40),
            ("record", "image", "registry.fly.io/unverified@sha256:" + "b" * 64),
            ("record", "image", "registry.fly.io/virtualcrypto-staging:latest"),
            ("record", "image", CANDIDATE.replace("virtualcrypto-staging", "virtualcrypto-prod")),
        ]
        for target, key, value in cases:
            with self.subTest(key=key, value=value):
                run, record = copy.deepcopy(self.run), copy.deepcopy(self.record)
                (run if target == "run" else record)[key] = value
                with self.assertRaises(ValueError):
                    release.provenance(record, run, "staging", REPOSITORY)

    def test_failed_production_rollout_can_supply_a_recovery_snapshot(self):
        self.run.update(path=".github/workflows/release.yml", event="workflow_dispatch",
                        conclusion="failure")
        self.record.update(operation="promote", verified=False, source_sha="d" * 40,
                           observed={"123abc": "machine-version-1"},
                           image=CANDIDATE.replace("virtualcrypto-staging", "virtualcrypto-prod"))
        self.validate("production")


class DeploymentSafety(unittest.TestCase):
    def test_staged_database_secret_is_rejected(self):
        pending = secrets()
        next(s for s in pending if s["name"] == "DATABASE_URL")["status"] = "Staged"
        with self.assertRaisesRegex(ValueError, "DATABASE_URL"):
            release.preflight_secrets(pending)

    def test_missing_proxy_secret_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "VCRYPTO_WEBHOOK_PROXY_KEY"):
            release.preflight_secrets([s for s in secrets()
                                      if s["name"] != "VCRYPTO_WEBHOOK_PROXY_KEY"])

    def test_partial_rollout_accepts_old_and_candidate_images(self):
        old = machine()
        current = copy.deepcopy(old)
        current["image_ref"].update(repository="virtualcrypto-staging", digest="sha256:" + "b" * 64)
        saved = {"machines": [old], "secrets": secrets()}
        self.assertEqual(set(release.recovery_inventory(saved, [current], CANDIDATE, secrets())),
                         {"123abc"})
        release.recovery_inventory(saved, [old], CANDIDATE, secrets())

    def test_stale_recovery_inventory_and_secret_rotation_are_rejected(self):
        saved = {"machines": [machine()], "secrets": secrets()}
        cases = [([machine("d" * 64)], secrets()), ([], secrets())]
        rotated = secrets()
        rotated[0]["digest"] = "rotated"
        cases.append(([machine()], rotated))
        for inventory, credentials in cases:
            with self.subTest(inventory=inventory, credentials=credentials):
                with self.assertRaises(ValueError):
                    release.recovery_inventory(saved, inventory, CANDIDATE, credentials)

    def test_recovery_restores_legacy_command_without_migrations(self):
        old = machine()
        restored = copy.deepcopy(old)
        restored["config"]["image"] = release.reference(old)
        with tempfile.TemporaryDirectory() as directory:
            release.write(Path(directory) / "previous.json",
                          {"schema": 1, "app": "virtualcrypto-prod", "machines": [old],
                           "secrets": secrets()})
            release.write(Path(directory) / "release.json",
                          {"image": CANDIDATE, "observed": {"123abc": "machine-version-1"}})
            with patch.object(release, "machines", side_effect=[[old], [restored]]), \
                    patch.object(release, "fly", return_value=json.dumps(secrets())), \
                    patch.object(release, "machine_api", side_effect=[{"data": {"nonce": "lease"}}, {}, {}]) as api, \
                    patch.object(release, "urlopen") as response:
                response.return_value.__enter__.return_value.status = 200
                release.recover(argparse.Namespace(directory=directory, environment="production"))
                update = api.call_args_list[1]
                self.assertEqual(update.args, ("virtualcrypto-prod", "123abc"))
                config = update.kwargs["data"]["config"]
                self.assertEqual(config["init"], old["config"]["init"])
                self.assertEqual(config["image"], release.reference(old))
                self.assertEqual(update.kwargs["data"]["current_version"], "instance-1")
                self.assertEqual(update.kwargs["nonce"], "lease")

    def test_config_only_change_blocks_recovery_even_when_the_digest_matches(self):
        old = machine()
        current = copy.deepcopy(old)
        current["version"] = "a-later-config-version"
        with tempfile.TemporaryDirectory() as directory:
            release.write(Path(directory) / "previous.json",
                          {"schema": 1, "app": "virtualcrypto-prod", "machines": [old],
                           "secrets": secrets()})
            release.write(Path(directory) / "release.json",
                          {"image": CANDIDATE, "observed": {"123abc": old["version"]}})
            with patch.object(release, "machines", return_value=[current]), \
                    patch.object(release, "fly") as fly:
                with self.assertRaisesRegex(ValueError, "stale recovery"):
                    release.recover(argparse.Namespace(directory=directory, environment="production"))
                fly.assert_not_called()


class BuildIdentity(unittest.TestCase):
    def test_registry_copy_preserves_digest_and_removes_temporary_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            record = {"image": CANDIDATE.replace("virtualcrypto-staging", "virtualcrypto-prod"),
                      "staging_image": CANDIDATE, "run_id": "456", "run_attempt": "1"}
            release.write(Path(directory) / "release.json", record)
            env = {"FLY_STAGING_IMAGE_TOKEN": "source-secret", "FLY_API_TOKEN": "dest-secret"}
            for actual in ("b" * 64, "d" * 64):
                with self.subTest(digest=actual), patch.dict(os.environ, env, clear=True), \
                        patch.object(release.subprocess, "run"), \
                        patch.object(release, "command", side_effect=["copied", "sha256:" + actual]) as command:
                    if actual == "b" * 64:
                        release.mirror(argparse.Namespace(directory=directory))
                    else:
                        with self.assertRaisesRegex(ValueError, "digest differs"):
                            release.mirror(argparse.Namespace(directory=directory))
                    copy_args = command.call_args_list[0].args
                    self.assertIn("--preserve-digests", copy_args)
                    self.assertIn("--all", copy_args)
                    self.assertNotIn("source-secret", copy_args)
                    self.assertNotIn("dest-secret", copy_args)
                    self.assertFalse(Path(copy_args[copy_args.index("--src-authfile") + 1]).exists())

    def test_digest_is_taken_from_the_pushed_image_not_an_intermediate_layer(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "build.log"
            tag = "git-test-123-2"
            log.write_text("exporting manifest sha256:" + "d" * 64 + "\n"
                           "pushing manifest for registry.fly.io/virtualcrypto-staging:"
                           + tag + "@sha256:" + "b" * 64 + " 0.1s done\n")
            env = {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_SHA": SOURCE,
                   "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2"}
            with patch.dict(os.environ, env, clear=True):
                release.manifest(argparse.Namespace(log=str(log), tag=tag, directory=directory))
            record = release.read(Path(directory) / "release.json")
            self.assertEqual(record["image"], CANDIDATE)
            self.assertFalse(record["verified"])
            with self.assertRaises(ValueError):
                release.manifest(argparse.Namespace(log=str(log), tag="another-tag", directory=directory))


if __name__ == "__main__":
    unittest.main()
