"""Publish the tested assets using only the scoped Actions token."""
import json
import os
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request
from check_release import check

folder = Path("release-assets")
check(folder)
version = json.loads(Path("package.json").read_text())["version"]
tag = "v" + version
repo = os.environ["GITHUB_REPOSITORY"]
base = "https://api.github.com/repos/" + repo
headers = {"Authorization": "Bearer " + os.environ["GITHUB_TOKEN"], "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}


def api(url, method="GET", data=None, binary=False):
    local = dict(headers)
    if data is not None:
        local["Content-Type"] = "application/octet-stream" if binary else "application/json"
    request = urllib.request.Request(url, data=data if binary else json.dumps(data).encode() if data is not None else None,
                                     method=method, headers=local)
    with urllib.request.urlopen(request, timeout=180) as response:
        payload = response.read()
        return json.loads(payload) if payload else {}


try:
    release = api(base + "/releases/tags/" + tag)
except urllib.error.HTTPError as error:
    if error.code != 404:
        raise
    release = api(base + "/releases", "POST", {"tag_name": tag, "target_commitish": os.environ["GITHUB_SHA"],
                  "name": "CableScope " + version, "draft": True, "prerelease": False})

if not release["draft"]:
    raise RuntimeError("This version is already published. Bump the version instead of replacing public binaries.")
for path in sorted(folder.iterdir()):
    if path.suffix not in (".zip", ".txt"):
        continue
    existing = next((a for a in release["assets"] if a["name"] == path.name), None)
    if existing:
        api(base + "/releases/assets/" + str(existing["id"]), "DELETE")
    url = release["upload_url"].split("{")[0] + "?name=" + urllib.parse.quote(path.name)
    api(url, "POST", path.read_bytes(), binary=True)
    print("Uploaded", path.name)

body = Path("docs/RELEASE_NOTES.md").read_text(encoding="utf-8")
result = api(base + "/releases/" + str(release["id"]), "PATCH", {"draft": False, "body": body, "make_latest": "true"})
print("Published", result["html_url"])
