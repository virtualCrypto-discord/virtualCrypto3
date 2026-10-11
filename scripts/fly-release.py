#!/usr/bin/env python3
"""Release metadata, provenance checks and recovery for the Fly workflows."""

import argparse
import copy
import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen

APPS = {"staging": "virtualcrypto-staging", "production": "virtualcrypto-prod"}
IMAGE = re.compile(r"registry\.fly\.io/virtualcrypto-(?:staging|prod)@sha256:[0-9a-f]{64}\Z")
SHA = re.compile(r"[0-9a-f]{40}\Z")
REQUIRED_SECRETS = {
    "DATABASE_URL", "VCRYPTO_API_JWT_SECRET_KEY", "VCRYPTO_BOT_TOKEN",
    "VCRYPTO_CLIENT_ID", "VCRYPTO_CLIENT_SECRET", "VCRYPTO_PUBLIC_KEY",
    "VCRYPTO_WEBHOOK_PROXY_CERT", "VCRYPTO_WEBHOOK_PROXY_KEY",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n")


def command(*args):
    return subprocess.check_output(args, text=True)


def fly(*args):
    return command(os.environ.get("FLYCTL", "flyctl"), *args)


def machine_api(app, path, *, method="POST", data=None, nonce=None):
    token = os.environ.get("FLY_API_TOKEN") or fly("auth", "token").strip()
    headers = {"Authorization": "Bearer " + token, "Content-Type": "application/json"}
    if nonce:
        headers["fly-machine-lease-nonce"] = nonce
    request = Request(f"https://api.machines.dev/v1/apps/{app}/machines/{path}",
                      data=json.dumps(data).encode() if data is not None else None,
                      headers=headers, method=method)
    with urlopen(request, timeout=60) as response:
        return json.load(response)


def machines(app):
    state = json.loads(fly("status", "--app", app, "--json"))
    require(state["Name"] == app, "Unexpected Fly app")
    result = state.get("Machines", [])
    require(result, "No application Machines found")
    require(all(m["config"].get("env", {}).get("FLY_PROCESS_GROUP") == "app"
                for m in result), "Unexpected process group; inspect the inventory")
    return result


def reference(machine):
    ref = machine["image_ref"]
    digest = ref["digest"]
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", digest), "Invalid Machine digest")
    return f'{ref["registry"]}/{ref["repository"]}@{digest}'


def secret_fingerprints(secrets):
    return {s["name"]: s["digest"] for s in secrets}


def preflight_secrets(secrets):
    names = {s["name"] for s in secrets}
    require(REQUIRED_SECRETS <= names,
            "Missing Fly secrets: " + ", ".join(sorted(REQUIRED_SECRETS - names)))
    pending = [s["name"] for s in secrets if s.get("status") != "Deployed"]
    require(not pending, "Resolve pending/partial Fly secrets before releasing: "
            + ", ".join(pending))


def provenance(record, run, kind, repository):
    require(record.get("schema") == 1, "Unsupported release record")
    require(record.get("repository") == repository, "Artifact repository mismatch")
    require(record.get("run_id") == str(run["id"]), "Artifact run ID mismatch")
    require(record.get("run_attempt") == str(run["run_attempt"]),
            "Artifact run attempt mismatch")
    require(record.get("workflow_sha") == run["head_sha"], "Workflow SHA mismatch")
    require(SHA.fullmatch(record.get("source_sha", "")), "Invalid source revision")
    require(IMAGE.fullmatch(record.get("image", "")), "Invalid candidate image")
    require(run["head_branch"] == "main" and run["status"] == "completed",
            "Select a completed main workflow run")
    require(run["head_repository"]["full_name"] == repository,
            "Workflow originated in another repository")
    require(run["event"] in ("push", "workflow_dispatch"), "Unsupported workflow event")
    if kind == "staging":
        require(record["image"].startswith("registry.fly.io/virtualcrypto-staging@"),
                "Staging artifact must identify the staging repository")
        require(run["path"] == ".github/workflows/ci.yml", "Select a ci workflow run")
        require(run["conclusion"] == "success", "CI run did not succeed")
        require(record.get("operation") == "staging" and record.get("verified") is True,
                "Staging artifact has not passed deployment and smoke checks")
        require(record["source_sha"] == run["head_sha"], "CI source SHA mismatch")
    else:
        require(record["image"].startswith("registry.fly.io/virtualcrypto-prod@"),
                "Production artifact must identify the production repository")
        require(run["path"] == ".github/workflows/release.yml",
                "Select a production release workflow run")
        require(record.get("operation") == "promote", "Select a promotion, not a recovery")
        require(record.get("observed"), "Release has no post-deploy Machine inventory")


def outputs(**values):
    if path := os.environ.get("GITHUB_OUTPUT"):
        with open(path, "a") as out:
            for key, value in values.items():
                out.write(f"{key}={value}\n")


def identity():
    return {
        "schema": 1,
        "repository": os.environ["GITHUB_REPOSITORY"],
        "workflow_sha": os.environ["GITHUB_SHA"],
        "run_id": os.environ["GITHUB_RUN_ID"],
        "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
    }


def manifest(args):
    log = re.sub(r"\x1b\[[0-9;]*m", "", Path(args.log).read_text())
    tag = f"registry.fly.io/virtualcrypto-staging:{args.tag}"
    digests = set(re.findall(r"pushing manifest for " + re.escape(tag)
                             + r"@(sha256:[0-9a-f]{64})", log))
    require(len(digests) == 1, "Build log must identify one pushed candidate manifest")
    record = identity() | {
        "source_sha": os.environ["GITHUB_SHA"], "operation": "staging",
        "image": "registry.fly.io/virtualcrypto-staging@" + digests.pop(),
        "verified": False,
    }
    write(Path(args.directory) / "release.json", record)
    outputs(image=record["image"])


def resolve(args):
    require(re.fullmatch(r"[1-9][0-9]*", args.run_id), "Run ID must be a positive integer")
    repo = os.environ["GITHUB_REPOSITORY"]
    run = json.loads(command("gh", "api", f"repos/{repo}/actions/runs/{args.run_id}"))
    directory = Path(args.directory)
    directory.mkdir(parents=True, exist_ok=True)
    # run_attempt disambiguates artifacts when the same run is re-executed.
    artifact = f'{args.kind}-release-{run["run_attempt"]}'
    command("gh", "run", "download", args.run_id, "--repo", repo,
            "--name", artifact, "--dir", str(directory))
    record = read(directory / "release.json")
    provenance(record, run, args.kind, repo)
    if args.kind == "production":
        require(read(directory / "previous.json")["app"] == APPS["production"],
                "Recovery snapshot app mismatch")
    outputs(source_sha=record["source_sha"], image=record["image"])


def promotion(args):
    staged = read(Path(args.directory) / "release.json")
    record = staged | identity() | {
        "operation": "promote", "verified": False,
        "ci_run_id": staged["run_id"], "ci_run_attempt": staged["run_attempt"],
        "staging_image": staged["image"],
        "image": "registry.fly.io/virtualcrypto-prod@" + staged["image"].split("@")[1],
    }
    write(Path(args.directory) / "release.json", record)
    outputs(image=record["image"])


def mirror(args):
    record = read(Path(args.directory) / "release.json")
    require(IMAGE.fullmatch(record["image"]) and IMAGE.fullmatch(record["staging_image"]),
            "Invalid mirror image reference")
    digest = record["staging_image"].split("@")[1]
    require(record["image"] == "registry.fly.io/virtualcrypto-prod@" + digest,
            "Promotion must preserve the staging digest")
    tag = f'promote-{record["run_id"]}-{record["run_attempt"]}'
    target = "registry.fly.io/virtualcrypto-prod:" + tag
    with tempfile.TemporaryDirectory(prefix="fly-registry-auth-") as directory:
        policy = Path(directory) / "policy.json"
        # Trust is the verified CI digest over TLS; these images are not signed.
        write(policy, {"default": [{"type": "insecureAcceptAnything"}]})
        auth = []
        for name in ("FLY_STAGING_IMAGE_TOKEN", "FLY_API_TOKEN"):
            token = os.environ.get(name)
            require(token, "Missing registry credential: " + name)
            config = Path(directory) / name
            config.mkdir()
            env = os.environ.copy()
            env.update(FLY_API_TOKEN=token, DOCKER_CONFIG=str(config))
            subprocess.run([os.environ.get("FLYCTL", "flyctl"), "auth", "docker"],
                           env=env, check=True, stdout=subprocess.DEVNULL)
            auth.append(str(config / "config.json"))
        command("skopeo", "--policy", str(policy), "copy", "--all", "--preserve-digests",
                "--src-authfile", auth[0], "--dest-authfile", auth[1],
                "docker://" + record["staging_image"], "docker://" + target)
        actual = command("skopeo", "inspect", "--authfile", auth[1],
                         "--format", "{{.Digest}}", "docker://" + target).strip()
        require(actual == digest, "Copied image digest differs from the staging digest")
    print("Verified production registry copy: " + record["image"])


def snapshot(args):
    directory = Path(args.directory)
    directory.mkdir(parents=True, exist_ok=True)
    app = APPS[args.environment]
    inventory = machines(app)
    secrets = json.loads(fly("secrets", "list", "--app", app, "--json"))
    # Fly-injected secrets are absent from Machine config. Refuse a plaintext copy.
    names = {s["name"] for s in secrets}
    for machine in inventory:
        require(not names.intersection(machine["config"].get("env", {})),
                "Machine config contains plaintext secret names; do not upload it")
    config = json.loads(fly("config", "show", "--app", app))
    require(config.get("app") == app, "Fly configuration app mismatch")
    require(not names.intersection(config.get("env", {})),
            "Fly config contains plaintext secret names; do not upload it")
    saved = {"schema": 1, "app": app, "machines": inventory, "secrets": secrets}
    write(directory / "previous.json", saved)
    write(directory / "fly.before.json", config)
    preflight_secrets(secrets)


def verify(args):
    path = Path(args.directory) / "release.json"
    record = read(path)
    require(IMAGE.fullmatch(record["image"]), "Invalid candidate image")
    app = APPS[args.environment]
    deadline = time.monotonic() + 120
    while True:
        inventory = machines(app)
        ready = all(reference(m) == record["image"] and m["state"] == "started"
                    and m.get("checks")
                    and all(c["status"] == "passing" for c in m["checks"])
                    for m in inventory)
        if ready:
            break
        require(time.monotonic() < deadline, "Machine digest/state/health verification failed")
        time.sleep(5)
    record["verified"] = True
    record["machines"] = [{"id": m["id"], "image": reference(m)} for m in inventory]
    write(path, record)


def recovery_inventory(saved, current, candidate, secrets):
    require(secret_fingerprints(saved["secrets"]) == secret_fingerprints(secrets),
            "Secrets changed since the release; recovery requires operator reconciliation")
    preflight_secrets(secrets)
    old = {m["id"]: m for m in saved["machines"]}
    require(set(old) == {m["id"] for m in current},
            "Machine inventory changed; inspect it before recovery")
    for machine in current:
        require(reference(machine) in (candidate, reference(old[machine["id"]])),
                "Another release changed a Machine; refusing stale recovery")
    return old


def observe(args):
    path = Path(args.directory) / "release.json"
    record = read(path)
    record["observed"] = {m["id"]: m["version"] for m in machines(APPS[args.environment])}
    write(path, record)


def recover(args):
    directory = Path(args.directory)
    saved = read(directory / "previous.json")
    record = read(directory / "release.json")
    app = APPS[args.environment]
    require(saved["schema"] == 1 and saved["app"] == app, "Recovery snapshot mismatch")
    require(IMAGE.fullmatch(record.get("image", "")), "Invalid candidate image")
    current = machines(app)
    require(record.get("observed") == {m["id"]: m["version"] for m in current},
            "Machine configuration changed after this run; refusing stale recovery")
    secrets = json.loads(fly("secrets", "list", "--app", app, "--json"))
    old = recovery_inventory(saved, current, record["image"], secrets)
    live = {m["id"]: m for m in current}
    for machine in old.values():
        config = copy.deepcopy(machine["config"])
        config["image"] = reference(machine)
        path = directory / f'recover-{machine["id"]}.json'
        write(path, config)
        # Use a complete API config: flyctl's config merge retains a newer CMD
        # when the saved init is empty. The lease and version guard prevent races.
        print(f'Restoring {machine["id"]} to {config["image"]}', flush=True)
        lease_path = machine["id"] + "/lease"
        lease = machine_api(app, lease_path, data={"ttl": 300, "description": "release recovery"})
        nonce = lease["data"]["nonce"]
        try:
            machine_api(app, machine["id"], nonce=nonce, data={
                "config": config, "current_version": live[machine["id"]]["instance_id"],
                "skip_launch": False,
            })
        finally:
            machine_api(app, lease_path, method="DELETE", nonce=nonce)
    deadline = time.monotonic() + 300
    while True:
        restored = machines(app)
        if all(reference(m) == reference(old[m["id"]]) and m["state"] == "started"
               and all(c["status"] == "passing" for c in (m.get("checks") or []))
               for m in restored):
            break
        require(time.monotonic() < deadline, "Recovered Machines did not become ready")
        time.sleep(5)
    for machine in restored:
        before = old[machine["id"]]
        require(reference(machine) == reference(before), "Recovery digest mismatch")
        for field in ("env", "init", "guest", "services", "restart"):
            require(machine["config"].get(field) == before["config"].get(field),
                    "Recovery runtime config mismatch: " + field)
    # Legacy production has no /health; verify its public root after restoration.
    deadline = time.monotonic() + 120
    while True:
        try:
            with urlopen(f"https://{app}.fly.dev/", timeout=30) as response:
                require(response.status == 200, "Recovered app root did not return HTTP 200")
            break
        except (URLError, TimeoutError):
            require(time.monotonic() < deadline, "Recovered app root is unavailable")
            time.sleep(2)
    for machine in old.values():
        if machine["state"] == "stopped":
            fly("machine", "stop", machine["id"], "--app", app)
    write(directory / "recovery.json", {"app": app, "restored": list(old)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="command", required=True)
    for name in ("manifest", "resolve", "promotion", "mirror", "snapshot", "verify", "observe", "recover"):
        sub = subs.add_parser(name)
        sub.add_argument("--directory", required=True)
        if name == "manifest":
            sub.add_argument("--log", required=True)
            sub.add_argument("--tag", required=True)
        if name == "resolve":
            sub.add_argument("--kind", choices=APPS, required=True)
            sub.add_argument("--run-id", required=True)
        if name in ("snapshot", "verify", "observe", "recover"):
            sub.add_argument("--environment", choices=APPS, required=True)
        sub.set_defaults(handler=globals()[name])
    args = parser.parse_args()
    try:
        args.handler(args)
    except (ValueError, KeyError, URLError, TimeoutError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release check failed: {error}\n")


if __name__ == "__main__":
    main()
