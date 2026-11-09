#!/usr/bin/env python3
"""Pin the shipped AppImage runtime and retain its grants and rebuild sources."""
import argparse
import json
import pathlib
import shutil
import tarfile
from build_ffmpeg import fetch, sha
ROOT=pathlib.Path(__file__).resolve().parents[2]

def member_bytes(archive, relative):
    with tarfile.open(archive) as source:
        matches=[m for m in source if m.isfile() and m.name.split('/',1)[-1]==relative]
        if len(matches)!=1 or matches[0].size>2*1024*1024:
            raise ValueError('missing or unbounded runtime source notice/recipe: '+relative)
        return source.extractfile(matches[0]).read()

def bundle(arch, resources, cache):
    if arch not in ('x86_64','aarch64'):
        raise ValueError('unsupported AppImage runtime architecture')
    resources,cache=pathlib.Path(resources),pathlib.Path(cache)
    cache.mkdir(parents=True,exist_ok=True)
    spec=json.loads((ROOT/'packaging/appimage-runtime.json').read_text())
    runtime=spec['runtimes'][arch]
    binary=cache/runtime['file'];fetch(runtime['url'],binary,runtime['sha256'])
    if binary.stat().st_size!=runtime['bytes']:
        raise ValueError('AppImage runtime size mismatch')
    source_dir=resources/'sources/appimage-runtime';source_dir.mkdir(parents=True,exist_ok=True)
    notices=resources/'licenses/appimage-runtime';notices.mkdir(parents=True,exist_ok=True)
    retained=[]
    for component in spec['sources']:
        archive=cache/component['file'];fetch(component['url'],archive,component['sha256'])
        if archive.stat().st_size!=component['bytes']:
            raise ValueError('AppImage corresponding-source size mismatch')
        shutil.copy2(archive,source_dir/archive.name)
        for relative in component['notices']:
            data=member_bytes(archive,relative)
            target=notices/component['package']/relative
            target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(data)
            retained.append({'file':target.relative_to(resources).as_posix(),'source':component['file'],'source_member':relative,'sha256':sha(target)})
        if component['package']=='type2-runtime':
            for relative in ('BUILD.md','scripts/build-runtime.sh','scripts/chroot/chroot_build.sh','scripts/common/install-dependencies.sh','scripts/docker/Dockerfile','scripts/docker/build-with-docker.sh','src/runtime/Makefile','patches/libfuse/mount.c.diff'):
                target=notices/'build'/relative;target.parent.mkdir(parents=True,exist_ok=True)
                target.write_bytes(member_bytes(archive,relative))
                retained.append({'file':target.relative_to(resources).as_posix(),'source':component['file'],'source_member':relative,'sha256':sha(target)})
    record=dict(spec,selected_architecture=arch,supplied_runtime_sha256=runtime['sha256'],retained_notices_and_recipe=retained)
    # Alpine recipe URLs/hashes are historical context only. No unrelated aports
    # scripts or utility code enter the release, and object-level proof is not claimed.
    (resources/'appimage-runtime-manifest.json').write_text(json.dumps(record,indent=2)+'\n')
    (notices/'SOURCE_AVAILABILITY.md').write_text('''# AppImage runtime source and replacement

This AppImage uses checksum-pinned type2-runtime 20251108. Its runtime is MIT
licensed and statically links LGPL 2.1 libfuse3 plus the permissive components
listed in appimage-runtime-manifest.json. Full original license texts accompany
this notice. Zstandard is used under its BSD license.

The exact runtime source, FUSE 3.15.0 source, upstream build scripts and FUSE
mount.c patch are included under sources/appimage-runtime/ and this directory's
build/ tree. These provide application and library source for rebuilding and
relinking the runtime with a modified libfuse. Follow the upstream BUILD.md and
scripts/docker/build-with-docker.sh; set src/runtime/version to the recorded
runtime commit URL. The additional versioned upstream dependency archives are
included alongside them. See their build instructions and retained notices.

To replace the runtime, extract the AppImage application payload and compose it
with appimagetool --runtime-file YOUR_REBUILT_RUNTIME. Any NLE does not restrict
modification or reverse engineering for debugging those modifications. The
original source and its redistribution terms remain available within the package.

Alpine recipe references in the manifest are historical build context, with
source URLs and checksums. Expired upstream build logs prevent proving every
permissive static dependency's original object-level package revision or a
byte-reproducible build. They do not change the exact runtime/FUSE source pins.
Alpine recipes and unrelated utility code are not redistributed here.
''')
    return binary

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--arch',required=True);parser.add_argument('--resources',required=True);parser.add_argument('--cache',required=True)
    args=parser.parse_args();print(bundle(args.arch,args.resources,args.cache))
