#!/usr/bin/env python3
"""Build the pinned, local-files-only LGPL backend; never copy a host FFmpeg."""
import argparse, hashlib, json, os, pathlib, shutil, subprocess, sys, tarfile, urllib.request
import http.client
import tempfile
import time
import urllib.error
from collections import deque
ROOT = pathlib.Path(__file__).resolve().parents[2]
def sha(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''): h.update(chunk)
    return h.hexdigest()
def fetch(url, dest, expected, *, offline=False):
    if dest.exists():
        if sha(dest) != expected:
            raise ValueError('FFmpeg source checksum mismatch')
        return
    if offline:
        raise FileNotFoundError(f'verified FFmpeg source archive is missing: {dest}')

    with tempfile.TemporaryDirectory(dir=dest.parent) as temporary:
        staged = pathlib.Path(temporary) / dest.name
        for delay in (2, 5, None):
            try:
                with urllib.request.urlopen(url, timeout=30) as response, staged.open('wb') as output:
                    shutil.copyfileobj(response, output)
                break
            except urllib.error.HTTPError as error:
                if delay is None or not (error.code in (408, 429) or 500 <= error.code <= 599):
                    raise
                error.close()
            except (urllib.error.URLError, ConnectionError, TimeoutError, http.client.IncompleteRead):
                if delay is None:
                    raise
            print(f'FFmpeg source connection failed. Retrying in {delay} seconds.', flush=True)
            time.sleep(delay)
        if sha(staged) != expected:
            raise ValueError('FFmpeg source checksum mismatch')
        staged.replace(dest)


def source_archive(spec, directory, *, offline=False):
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / 'ffmpeg-8.1.tar.xz'
    fetch(spec['url'], archive, spec['sha256'], offline=offline)
    return archive


def build(target, output, work, *, offline_source=False):
    spec = json.loads((ROOT/'packaging/ffmpeg.json').read_text())
    output, work = pathlib.Path(output).resolve(), pathlib.Path(work).resolve()
    output.mkdir(parents=True, exist_ok=True); work.mkdir(parents=True, exist_ok=True)
    archive = source_archive(spec, work, offline=offline_source)
    source = work/'ffmpeg-8.1'
    if not source.exists():
        with tarfile.open(archive) as t: t.extractall(work, filter='data')
    config = list(spec['configure'])
    windows = 'windows' in target
    suffix = '.exe' if windows else ''
    if windows:
        # FFmpeg is built with MinGW/clang, independently from the MSVC Rust app.
        # Static compiler runtimes avoid a hidden dependency on an MSYS2 install.
        config += ['--target-os=mingw32', '--extra-ldflags=-static']
        if target.startswith('aarch64'):
            config += ['--arch=aarch64', '--cc=clang', '--cxx=clang++', '--ar=llvm-ar', '--nm=llvm-nm', '--ranlib=llvm-ranlib']
    env = dict(os.environ)
    env.update(SOURCE_DATE_EPOCH='1776384000')
    build_log = work/'build.log'
    try:
        with build_log.open('w') as log:
            programs = ['ffmpeg' + suffix, 'ffprobe' + suffix]
            for cmd in (['sh', './configure', *config], ['make', '-j', str(min(os.cpu_count() or 2, 8)), *programs]):
                subprocess.run(cmd, cwd=source, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    except (OSError, subprocess.CalledProcessError):
        for path in (build_log, source/'ffbuild'/'config.log'):
            if path.is_file():
                with path.open(encoding='utf-8', errors='replace') as log:
                    print(f'Last lines of {path.name}:', file=sys.stderr)
                    sys.stderr.writelines(deque(log, maxlen=40))
        raise
    tools = {}
    for name in ('ffmpeg', 'ffprobe'):
        p = output/(name+suffix); shutil.copy2(source/(name+suffix), p)
        version = subprocess.check_output([str(p), '-version'], text=True)
        if not version.startswith(name+' version 8.1 '): raise ValueError('wrong backend version')
        for flag in ('--enable-gpl', '--enable-nonfree', '--enable-version3'):
            if flag in version: raise ValueError('forbidden backend license configuration')
        (output/(name+'-version.txt')).write_text(version)
        tools[name] = {'file': p.name, 'sha256': sha(p)}
    manifest = {'schema': 1, 'ffmpeg_version': spec['version'], 'target': target,
                'license': spec['license'], 'source': {'url': spec['url'], 'sha256': spec['sha256']},
                'configure': config, 'tools': tools}
    (output/'backend-manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
    notices = output/'licenses'/'ffmpeg'; notices.mkdir(parents=True, exist_ok=True)
    for name in ('LICENSE.md', 'COPYING.LGPLv2.1'):
        shutil.copy2(source/name, notices/name)
    if 'windows' in target:
        # Preserve a toolchain notice superset, including MinGW/GCC runtime
        # exceptions or LLVM support notices. Compiler tools are not shipped.
        prefix=os.environ.get('MINGW_PREFIX')
        if not prefix:
            raise ValueError('Windows compiler runtime notices are unavailable')
        if os.name=='nt':
            prefix=subprocess.check_output(['cygpath', '-w', prefix], text=True).strip()
        compiler_notices=pathlib.Path(prefix)/'share/licenses'
        if not compiler_notices.is_dir():
            raise ValueError('Windows compiler runtime notices are unavailable')
        destination=output/'licenses/windows-toolchain'
        shutil.copytree(compiler_notices,destination,dirs_exist_ok=True)
        (destination/'SCOPE.txt').write_text('Original MSYS2 toolchain notice superset for static compiler support objects. Tool executables are not redistributed. This does not change the FFmpeg LGPL configuration.\n')
    shutil.copy2(ROOT/'packaging/ffmpeg.json', notices/'source.json')
    shutil.copy2(pathlib.Path(__file__), notices/'build_ffmpeg.py')
    shutil.copy2(archive, output/'ffmpeg-8.1-source.tar.xz')
    shutil.copy2(source/'config.h', output/'ffmpeg-config.h')
    shutil.copy2(work/'build.log', output/'ffmpeg-build.log')
    print(json.dumps({'target': target, 'backend': str(output), 'tools': tools}))
if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--target')
    parser.add_argument('--output')
    parser.add_argument('--work', required=True, type=pathlib.Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--prepare-source', action='store_true')
    mode.add_argument('--offline-source', action='store_true')
    args = parser.parse_args()
    if args.prepare_source:
        if args.target or args.output:
            parser.error('--prepare-source only takes --work')
        spec = json.loads((ROOT / 'packaging/ffmpeg.json').read_text())
        print(source_archive(spec, args.work))
    else:
        if not args.target or not args.output:
            parser.error('--target and --output are required to build')
        build(args.target, args.output, args.work, offline_source=args.offline_source)
