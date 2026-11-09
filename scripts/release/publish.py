#!/usr/bin/env python3
"""Stage a new immutable version, verify downloaded asset bytes, then publish.

Only the release job invokes this script. It never pushes Git history, changes
repository visibility, or overwrites an already published version.
"""
import argparse,json,os,pathlib,subprocess,tempfile
from build_ffmpeg import sha
from verify_candidate import verify_all
def gh(*args):return subprocess.check_output(['gh',*args],text=True)
def validate_existing(existing,head):
    if len(existing)>1:raise ValueError('ambiguous release version')
    if existing:
        if not existing[0]['draft']:raise ValueError('published version is immutable; use a new version')
        if existing[0]['target_commitish']!=head:raise ValueError('draft belongs to a different source revision')

def publish(candidates,version,make_public):
    assets=verify_all(candidates,version);tag='v'+version
    repo=os.environ['GITHUB_REPOSITORY'];head=os.environ['GITHUB_SHA']
    releases=json.loads(gh('api','--paginate',f'repos/{repo}/releases?per_page=100','--slurp'))
    existing=[r for page in releases for r in page if r['tag_name']==tag]
    validate_existing(existing,head)
    if make_public:
        if json.loads(gh('api',f'repos/{repo}'))['private']:raise ValueError('repository must already pass external privacy review and be public')
    if not existing:
        # Never attach a new release to an unreviewed tag that already exists.
        refs=json.loads(gh('api',f'repos/{repo}/git/matching-refs/tags/{tag}'))
        if any(r['ref']=='refs/tags/'+tag for r in refs):raise ValueError('version tag already exists without this reviewed draft')
        subprocess.run(['gh','release','create',tag,'--repo',repo,'--target',head,'--title','Any NLE '+tag,'--draft','--prerelease','--notes-file','RELEASE_NOTES.md',*[str(p) for p in assets]],check=True)
    with tempfile.TemporaryDirectory(prefix='any-nle-upload-verify-') as temp:
        subprocess.run(['gh','release','download',tag,'--repo',repo,'--dir',temp],check=True)
        expected={p.name:sha(p) for p in assets};downloaded={p.name:sha(p) for p in pathlib.Path(temp).iterdir() if p.is_file()}
        if expected!=downloaded:raise ValueError('staged release asset bytes differ; draft remains unpublished')
    if make_public:subprocess.run(['gh','release','edit',tag,'--repo',repo,'--draft=false','--latest=false'],check=True)
    print(json.dumps({'tag':tag,'uploaded_sha256_verified':True,'published':make_public,'assets':len(assets)}))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--candidates',required=True);p.add_argument('--version',required=True);p.add_argument('--publish',choices=('true','false'),default='false');a=p.parse_args();publish(a.candidates,a.version,a.publish=='true')
