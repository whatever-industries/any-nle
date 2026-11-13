#!/usr/bin/env python3
"""Stage or resume a draft for the reviewed commit, then verify and publish."""

import argparse
from hashlib import sha256
import json
import os
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import urlencode

from build_ffmpeg import sha
from release_version import ROOT, validate_workflow
from verify_candidate import verify_all


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True)


def validate_existing(existing, head):
    if len(existing) > 1:
        raise ValueError("ambiguous release version")
    if existing:
        if not existing[0]["draft"]:
            raise ValueError("published version is immutable; use a new version")
        if existing[0]["target_commitish"] != head:
            raise ValueError("draft belongs to a different source revision")


def draft_for(repo, tag, head):
    pages = json.loads(gh("api", "--paginate", f"repos/{repo}/releases?per_page=100", "--slurp"))
    existing = [r for page in pages for r in page if r["tag_name"] == tag]
    validate_existing(existing, head)
    refs = json.loads(gh("api", f"repos/{repo}/git/matching-refs/tags/{tag}"))
    if any(r["ref"] == "refs/tags/" + tag for r in refs):
        raise ValueError("version tag already exists")
    return existing[0] if existing else None


def missing_assets(repo, draft, expected):
    pages = json.loads(gh("api", "--paginate",
                          f"repos/{repo}/releases/{draft['id']}/assets?per_page=100", "--slurp"))
    present = set()
    for page in pages:
        for asset in page:
            name = asset["name"]
            if name not in expected or name in present:
                raise ValueError(f"unexpected or duplicate draft asset: {name}")
            if asset["state"] != "uploaded" or asset.get("digest") != "sha256:" + expected[name]:
                raise ValueError(f"draft asset differs from the reviewed candidate: {name}")
            present.add(name)
    return expected.keys() - present


def require_reviewed_draft(draft, draft_body):
    if not draft["prerelease"]:
        raise ValueError("reviewed release draft must be a prerelease")
    if draft.get("body", "").rstrip() != draft_body.rstrip():
        raise ValueError("draft does not match the reviewed notes and asset set; use the original reviewed candidates")


def require_same_draft(repo, tag, head, draft_id, draft_body):
    draft = draft_for(repo, tag, head)
    if draft is None or draft["id"] != draft_id:
        raise ValueError("release draft changed during upload")
    require_reviewed_draft(draft, draft_body)
    return draft


def upload_url(repo, draft, name):
    endpoint = f"https://uploads.github.com/repos/{repo}/releases/{draft['id']}/assets"
    if draft.get("upload_url") != endpoint + "{?name,label}":
        raise ValueError("unexpected GitHub upload URL for the verified draft")
    return endpoint + "?" + urlencode({"name": name})


def publish(candidates, version, make_public):
    repo, head = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_SHA"]
    binding = validate_workflow(version, head, os.environ["PRIVATE_GATE"], head)
    assets = verify_all(candidates, version, expected_binding=binding)
    tag = "v" + version
    paths = {path.name: path for path in assets}
    expected = {name: sha(path) for name, path in paths.items()}
    notes = (ROOT / "RELEASE_NOTES.md").read_text(encoding="utf-8").rstrip() + "\n"
    # Bind even the assets that have not reached GitHub yet. The comment stays hidden in the release notes.
    manifest = json.dumps(dict(binding=binding, assets=expected), sort_keys=True, separators=(",", ":"))
    draft_body = notes + "\n<!-- anynle-release-assets-sha256: " + sha256(manifest.encode()).hexdigest() + " -->\n"
    if make_public and json.loads(gh("api", f"repos/{repo}"))["private"]:
        raise ValueError("repository must pass external privacy review and be public")

    draft = draft_for(repo, tag, head)
    if draft is None:
        with tempfile.TemporaryDirectory(prefix="any-nle-draft-notes-") as directory:
            draft_notes = Path(directory) / "notes.md"
            draft_notes.write_text(draft_body, encoding="utf-8", newline="\n")
            gh("release", "create", tag, "--repo", repo, "--target", head, "--title", "Any NLE " + tag,
               "--draft", "--prerelease", "--notes-file", str(draft_notes))
        draft = draft_for(repo, tag, head)
        if draft is None:
            raise ValueError("created release draft could not be read")
    draft_id = draft["id"]
    require_reviewed_draft(draft, draft_body)

    missing = missing_assets(repo, draft, expected)
    for name in sorted(missing):
        if sha(paths[name]) != expected[name]:
            raise ValueError(f"candidate changed before upload: {name}")
        draft = require_same_draft(repo, tag, head, draft_id, draft_body)
        # The asset endpoint rejects an occupied name and addresses this draft by ID.
        gh("api", "--method", "POST", upload_url(repo, draft, name),
           "-H", "Content-Type: application/octet-stream", "--input", str(paths[name]))

    draft = require_same_draft(repo, tag, head, draft_id, draft_body)
    if missing_assets(repo, draft, expected):
        raise ValueError("draft is missing reviewed assets; resume with the retained candidates")
    with tempfile.TemporaryDirectory(prefix="any-nle-upload-verify-") as directory:
        gh("release", "download", tag, "--repo", repo, "--dir", directory)
        downloaded = {p.name: sha(p) for p in Path(directory).iterdir() if p.is_file()}
        if downloaded != expected:
            raise ValueError("staged release asset bytes differ; draft remains unpublished")
    if make_public:
        draft = require_same_draft(repo, tag, head, draft_id, draft_body)
        if missing_assets(repo, draft, expected):
            raise ValueError("draft changed after download verification")
        # Address the audited release ID so a replaced draft cannot be published.
        published = json.loads(gh("api", "--method", "PATCH", f"repos/{repo}/releases/{draft_id}",
                                  "-F", "draft=false", "-f", "make_latest=false"))
        if (published["id"] != draft_id or published["draft"] or not published["prerelease"]
                or published["tag_name"] != tag or published["target_commitish"] != head):
            raise ValueError("published release does not match the verified draft")
    print(json.dumps(dict(tag=tag, uploaded_sha256_verified=True, published=make_public, assets=len(assets))))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidates", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--publish", choices=("true", "false"), default="false")
    args = parser.parse_args()
    publish(args.candidates, args.version, args.publish == "true")
