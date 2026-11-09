#!/usr/bin/env python3
"""Verify actual extracted packages, target architectures and six-job gate receipts."""
import argparse,hashlib,json,os,pathlib,shutil,struct,subprocess,tempfile,zipfile
from package import TARGETS
from build_ffmpeg import sha
ROOT=pathlib.Path(__file__).resolve().parents[2]
VERSION='0.1.0-alpha.1'
def architecture(path,target):
    b=path.read_bytes()[:4096]
    arm=target.startswith('aarch64')
    if 'windows' in target:
        if b[:2]!=b'MZ':raise ValueError('expected PE executable')
        offset=struct.unpack_from('<I',b,0x3c)[0]
        if offset+6>len(b) or b[offset:offset+4]!=b'PE\0\0':raise ValueError('invalid PE header')
        if struct.unpack_from('<H',b,offset+4)[0]!=(0xaa64 if arm else 0x8664):raise ValueError('wrong PE architecture')
    elif 'linux' in target:
        if b[:5]!=b'\x7fELF\x02':raise ValueError('expected 64-bit ELF')
        if struct.unpack_from('<H',b,18)[0]!=(183 if arm else 62):raise ValueError('wrong ELF architecture')
    else:
        if b[:4]!=b'\xcf\xfa\xed\xfe':raise ValueError('expected thin 64-bit Mach-O')
        if struct.unpack_from('<I',b,4)[0]!=(0x0100000c if arm else 0x01000007):raise ValueError('wrong Mach-O architecture')
def verify_package(target,directory,version):
    directory=pathlib.Path(directory);suffix,extension=TARGETS[target];stem=f'Any.NLE_{suffix}_v{version}'
    package=directory/(stem+extension);stage=directory/(stem+'.stage')
    if not package.is_file() or package.stat().st_size<10000:raise ValueError('missing or empty package')
    for p in (package,directory/(stem+'.provenance.tar.gz')):
        if (directory/(p.name+'.sha256')).read_text()!=sha(p)+'  '+p.name+'\n':raise ValueError('package checksum mismatch')
    with tempfile.TemporaryDirectory(prefix='any-nle-package-check-') as temp:
        temp=pathlib.Path(temp)
        if extension=='.zip':
            with zipfile.ZipFile(package) as z:
                for i in z.infolist():
                    parts=pathlib.PurePosixPath(i.filename).parts
                    if '..' in parts or i.filename.startswith('/'):raise ValueError('ZIP traversal')
            # Finder/ditto restore sequestered resource attributes; Python's
            # ZIP extractor drops signatures carried in those attributes.
            subprocess.run(['ditto','-x','-k',str(package.resolve()),str(temp)],check=True)
            extracted=temp;bindir=temp/'Any NLE.app/Contents/MacOS'
        elif extension=='.AppImage':
            subprocess.run([str(package.resolve()),'--appimage-extract'],cwd=temp,stdout=subprocess.DEVNULL,check=True)
            extracted=temp/'squashfs-root';bindir=extracted/'usr/bin'
        else:
            tool=shutil.which('7z') or r'C:\Program Files\7-Zip\7z.exe'
            subprocess.run([tool,'x','-y','-o'+str(temp),str(package.resolve())],stdout=subprocess.DEVNULL,check=True)
            extracted=temp;bindir=temp
        exe='.exe' if 'windows' in target else ''
        for p in stage.rglob('*'):
            if p.is_file():
                counterpart=extracted/p.relative_to(stage)
                if not counterpart.is_file() or sha(p)!=sha(counterpart):raise ValueError('extracted package differs from audited stage')
        m=json.loads((bindir/'backend-manifest.json').read_text())
        if m['target']!=target:raise ValueError('manifest target mismatch')
        for name in ('any-nle','any-nle-gui','ffmpeg','ffprobe'):
            p=bindir/(name+exe);architecture(p,target)
            if not exe:p.chmod(p.stat().st_mode|0o111)
            if name in m['tools'] and sha(p)!=m['tools'][name]['sha256']:raise ValueError('shipped backend hash mismatch')
        help_result=subprocess.run([str(bindir/('any-nle'+exe)),'--help'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
        if help_result.returncode or 'any-nle' not in help_result.stdout.lower():raise ValueError('packaged CLI smoke failed')
        for name in ('ffmpeg','ffprobe'):
            result=subprocess.check_output([str(bindir/(name+exe)),'-version'],text=True)
            if not result.startswith(name+' version 8.1 '):raise ValueError('packaged backend smoke failed')
            if any(f in result for f in ('--enable-gpl','--enable-version3','--enable-nonfree')):raise ValueError('packaged forbidden backend license')
        if 'apple' in target:subprocess.run(['codesign','--verify','--deep','--strict',str(temp/'Any NLE.app')],check=True)
    receipt={'schema':'anynle.candidate-verification/1','target':target,'version':version,'package':package.name,'sha256':sha(package),'extracted_bytes_match':True,'architecture_verified':True,'cli_smoke':True,'backend_smoke':True}
    pathlib.Path('candidate-verification.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt));return receipt

def verify_all(directory,version):
    directory=pathlib.Path(directory);seen=set();binding=None;files=[]
    for target,(suffix,ext) in TARGETS.items():
        candidate=directory/('any-nle-'+suffix)
        r=json.loads((candidate/'candidate-verification.json').read_text());p=candidate/f'Any.NLE_{suffix}_v{version}{ext}'
        if r['target']!=target or r['version']!=version or r['package']!=p.name or sha(p)!=r['sha256']:raise ValueError('candidate verification does not bind asset')
        if not all(r[k] for k in ('extracted_bytes_match','architecture_verified','cli_smoke','backend_smoke')):raise ValueError('candidate gate incomplete')
        public=json.loads((candidate/'public-acceptance.json').read_text())
        if public['profile']!='synthetic-only' or public['passed']<92 or public['private_corpus_qualification'] is not False:raise ValueError('public qualification gate invalid')
        smoke=json.loads((candidate/'backend-smoke.json').read_text())
        if smoke['target']!=target or not smoke['passed'] or len(smoke['synthetic_cases'])!=2:raise ValueError('synthetic backend smoke incomplete')
        audit=json.loads((candidate/'package-audit.json').read_text())
        if not audit['passed'] or audit['match_locations'] or any(not x.get('accepted_upstream_fixture',False) for x in audit.get('undecodable_archives',[])):raise ValueError('package privacy gate failed')
        gate=(candidate/'gate-binding.txt').read_text()
        if binding is None:binding=gate
        if gate!=binding:raise ValueError('candidate source/private receipt bindings differ')
        for asset in (p,candidate/f'Any.NLE_{suffix}_v{version}.provenance.tar.gz'):
            if (candidate/(asset.name+'.sha256')).read_text()!=sha(asset)+'  '+asset.name+'\n':raise ValueError('candidate checksum mismatch')
            files.append(asset)
        seen.add(target)
    if seen!=set(TARGETS):raise ValueError('all six candidates required')
    sums=directory/'SHA256SUMS';sums.write_text(''.join(sha(p)+'  '+p.name+'\n' for p in sorted(files,key=lambda p:p.name)))
    print(json.dumps({'schema':'anynle.six-platform-gate/1','targets':sorted(seen),'assets':len(files),'source_private_receipt_binding':binding}));return files+[sums]
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--target',choices=TARGETS);p.add_argument('--output');p.add_argument('--all');p.add_argument('--version',default=VERSION);a=p.parse_args()
    if a.version!=VERSION:raise ValueError('unreviewed version')
    if a.all:verify_all(a.all,a.version)
    elif a.target and a.output:verify_package(a.target,a.output,a.version)
    else:p.error('use --all or --target and --output')
