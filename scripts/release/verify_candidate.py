#!/usr/bin/env python3
"""Verify actual extracted packages, target architectures and six-job gate receipts."""
import argparse,hashlib,json,os,pathlib,plistlib,shutil,struct,subprocess,tempfile,zipfile
from audit import audit, load_known_upstream
from package import ICONS,TARGETS,verify_backend,verify_resolve_templates
from build_ffmpeg import sha
from backend_smoke import exercise as exercise_backend
import intel_resolve_smoke
ROOT=pathlib.Path(__file__).resolve().parents[2]
from release_version import gate_binding, release_version
INTEL_MAC = 'x86_64-apple-darwin'
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

def verify_launcher_icon(target, extracted, package):
    if 'apple' in target:
        contents = extracted/'Any NLE.app/Contents'
        icon = plistlib.loads((contents/'Info.plist').read_bytes()).get('CFBundleIconFile')
        if icon != 'any-nle.icns' or sha(contents/'Resources'/icon) != sha(ICONS/icon):
            raise ValueError('macOS bundle icon mismatch')
    elif 'linux' in target:
        icon = sha(ICONS/'any-nle.png')
        desktop = (extracted/'any-nle.desktop').read_text().splitlines()
        if 'Icon=any-nle' not in desktop or any(
            sha(extracted/name) != icon for name in ('any-nle.png', '.DirIcon')
        ):
            raise ValueError('AppImage launcher icon mismatch')
    else:
        ico = (ICONS/'any-nle.ico').read_bytes()
        reserved, kind, count = struct.unpack_from('<HHH', ico)
        if (reserved, kind) != (0, 1) or not 1 <= count <= 16:
            raise ValueError('invalid Windows icon source')
        images = []
        for i in range(count):
            size, start = struct.unpack_from('<II', ico, 6 + 16*i + 8)
            image = ico[start:start + size]
            if len(image) != size or not image.startswith(b'\x89PNG\r\n\x1a\n'):
                raise ValueError('invalid Windows icon image')
            images.append(image)
        # RC and NSIS retain the exact PNG icon payloads in the PE resources.
        for path in (package, extracted/'any-nle-gui.exe'):
            binary = path.read_bytes()
            if not all(image in binary for image in images):
                raise ValueError('Windows launcher icon mismatch: ' + path.name)

def verify_package(target,directory,version,*,receipt_path=pathlib.Path('candidate-verification.json'),
                   resolve_source_probe=None, audit_receipt=None):
    version = release_version(version)
    receipt_path = pathlib.Path(receipt_path)
    audit_receipt = (receipt_path.with_name('package-audit.json') if audit_receipt is None
                     else pathlib.Path(audit_receipt))
    if audit_receipt.resolve() == receipt_path.resolve():
        raise ValueError('candidate and privacy audit need separate receipt paths')
    if target == INTEL_MAC and resolve_source_probe is None:
        raise ValueError('Intel macOS package verification requires --resolve-source-probe')
    directory=pathlib.Path(directory);suffix,extension=TARGETS[target];stem=f'Any.NLE_{suffix}_v{version}'
    package=directory/(stem+extension);stage=directory/(stem+'.stage')
    if not package.is_file() or package.stat().st_size<10000:raise ValueError('missing or empty package')
    staged_files = sorted(p for p in stage.rglob('*') if p.is_file())
    if not staged_files:
        raise ValueError('missing or empty package staging directory')
    package_sha = sha(package)
    provenance = directory / (stem + '.provenance.tar.gz')
    provenance_sha = sha(provenance)
    for path, digest in ((package, package_sha), (provenance, provenance_sha)):
        if (directory/(path.name+'.sha256')).read_text()!=digest+'  '+path.name+'\n':raise ValueError('package checksum mismatch')
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
            resources = bindir.parent / 'Resources'
        elif extension=='.AppImage':
            subprocess.run([str(package.resolve()),'--appimage-extract'],cwd=temp,stdout=subprocess.DEVNULL,check=True)
            extracted=temp/'squashfs-root';bindir=extracted/'usr/bin'
            resources = extracted / 'usr/share/any-nle'
        else:
            tool=shutil.which('7z') or r'C:\Program Files\7-Zip\7z.exe'
            subprocess.run([tool,'x','-y','-o'+str(temp),str(package.resolve())],stdout=subprocess.DEVNULL,check=True)
            extracted=temp;bindir=temp
            resources = extracted / 'resources'
        exe='.exe' if 'windows' in target else ''
        for staged in staged_files:
            counterpart = extracted / staged.relative_to(stage)
            if not counterpart.is_file() or sha(staged) != sha(counterpart):
                raise ValueError('extracted package differs from staging directory')
        known = load_known_upstream(ROOT / 'packaging/audit-known-upstream-fixtures.json')
        audit_report = audit((extracted, provenance), known_upstream=known)
        audit_bytes = (json.dumps(audit_report, indent=2) + '\n').encode('utf-8')
        audit_receipt.write_bytes(audit_bytes)
        if not audit_report['passed']:
            raise ValueError('extracted package or provenance failed privacy audit')
        verify_launcher_icon(target,extracted,package)
        if 'apple' in target:
            verify_resolve_templates(resources / 'resolve-templates')
            info = plistlib.loads((bindir.parent/'Info.plist').read_bytes())
            numeric_version = version.split('-', 1)[0]
            if any(info.get(key) != numeric_version for key in ('CFBundleShortVersionString', 'CFBundleVersion')):
                raise ValueError('macOS bundle version mismatch')
        verify_backend(bindir, target, source_directory=resources)
        for name in ('any-nle','any-nle-gui','ffmpeg','ffprobe'):
            p=bindir/(name+exe);architecture(p,target)
            if not exe:p.chmod(p.stat().st_mode|0o111)
        if not exe:
            gui_version = subprocess.check_output([str(bindir/'any-nle-gui'), '--version'], text=True)
            if gui_version.strip() != f'Any NLE {version}':
                raise ValueError('packaged GUI version differs from the release version')
        help_result=subprocess.run([str(bindir/('any-nle'+exe)),'--help'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
        if help_result.returncode or 'any-nle' not in help_result.stdout.lower():raise ValueError('packaged CLI smoke failed')
        for name in ('ffmpeg','ffprobe'):
            result=subprocess.check_output([str(bindir/(name+exe)),'-version'],text=True)
            if not result.startswith(name+' version 8.1 '):raise ValueError('packaged backend smoke failed')
            if any(f in result for f in ('--enable-gpl','--enable-version3','--enable-nonfree')):raise ValueError('packaged forbidden backend license')
        if 'apple' in target:subprocess.run(['codesign','--verify','--deep','--strict',str(temp/'Any NLE.app')],check=True)
        backend_media = exercise_backend(bindir, target)
        if target == INTEL_MAC:
            intel_smoke = intel_resolve_smoke.verify(temp/'Any NLE.app', resolve_source_probe, temp/'resolve-smoke')
    if sha(package) != package_sha:
        raise ValueError('package changed during verification')
    if sha(provenance) != provenance_sha:
        raise ValueError('provenance changed during verification')
    receipt={'schema':'anynle.candidate-verification/1','target':target,'version':version,'package':package.name,'sha256':package_sha,'extracted_bytes_match':True,'architecture_verified':True,'launcher_icon_verified':True,'cli_smoke':True,'backend_smoke':True,'backend_media':backend_media}
    receipt['provenance_sha256'] = provenance_sha
    receipt['package_audit_sha256'] = hashlib.sha256(audit_bytes).hexdigest()
    if target == INTEL_MAC:
        receipt['avid_resolve_smoke'] = intel_smoke
    pathlib.Path(receipt_path).write_text(json.dumps(receipt,indent=2)+'\n')
    print(json.dumps(receipt))
    return receipt

def verify_all(directory,version,*,expected_binding=None):
    version = release_version(version)
    directory=pathlib.Path(directory);seen=set();binding=None;files={}
    for target,(suffix,ext) in TARGETS.items():
        candidate=directory/('any-nle-'+suffix)
        r=json.loads((candidate/'candidate-verification.json').read_text());p=candidate/f'Any.NLE_{suffix}_v{version}{ext}'
        if r['target'] != target or r['version'] != version or r['package'] != p.name:
            raise ValueError('candidate verification does not bind asset')
        package_sha = sha(p)
        if package_sha != r['sha256']:
            raise ValueError('candidate verification does not bind asset')
        if not all(r[k] for k in ('extracted_bytes_match','architecture_verified','launcher_icon_verified','cli_smoke','backend_smoke')):raise ValueError('candidate gate incomplete')
        if target == INTEL_MAC and r.get('avid_resolve_smoke') != intel_resolve_smoke.expected_receipt():
            raise ValueError('Intel Avid-to-Resolve package smoke missing or mismatched')
        public=json.loads((candidate/'public-acceptance.json').read_text())
        if public['profile']!='synthetic-only' or public['passed']<92 or public['private_corpus_qualification'] is not False:raise ValueError('public qualification gate invalid')
        smoke = r.get('backend_media')
        expected_cases = [
            {'codec': 'dnxhd', 'container': 'mxf', 'decoded_picture_and_pcm_equal': True},
            {'codec': 'prores_ks', 'container': 'mov', 'decoded_picture_and_pcm_equal': True},
            {'codec': 'png', 'container': 'png', 'decoded_rgba_equal': True},
        ]
        if smoke != {'schema': 'anynle.backend-smoke/1', 'target': target,
                     'synthetic_cases': expected_cases, 'passed': True}:
            raise ValueError('extracted synthetic backend smoke incomplete')
        audit_bytes = (candidate / 'package-audit.json').read_bytes()
        if hashlib.sha256(audit_bytes).hexdigest() != r.get('package_audit_sha256'):
            raise ValueError('candidate verification does not bind privacy audit')
        audit_report = json.loads(audit_bytes)
        if (audit_report.get('schema') != 'anynle.privacy-audit/1'
                or audit_report.get('passed') is not True
                or audit_report['match_locations']
                or any(not item.get('accepted_upstream_fixture', False)
                       for item in audit_report['undecodable_archives'])):
            raise ValueError('package privacy gate failed')
        gate=(candidate/'gate-binding.txt').read_text()
        if expected_binding is not None and gate != expected_binding:
            raise ValueError('candidate does not match reviewed source and receipt')
        if binding is None:binding=gate
        if gate!=binding:raise ValueError('candidate source/private receipt bindings differ')
        provenance = candidate / f'Any.NLE_{suffix}_v{version}.provenance.tar.gz'
        provenance_sha = sha(provenance)
        if provenance_sha != r.get('provenance_sha256'):
            raise ValueError('candidate verification does not bind provenance')
        for asset, digest in ((p, package_sha), (provenance, provenance_sha)):
            if (candidate/(asset.name+'.sha256')).read_text()!=digest+'  '+asset.name+'\n':raise ValueError('candidate checksum mismatch')
            files[asset] = digest
        seen.add(target)
    if seen!=set(TARGETS):raise ValueError('all six candidates required')
    sums = directory / 'SHA256SUMS'
    checksums = ''.join(
        files[path] + '  ' + path.name + '\n' for path in sorted(files, key=lambda path: path.name)
    ).encode('ascii')
    sums.write_bytes(checksums)
    print(json.dumps({'schema':'anynle.six-platform-gate/1','targets':sorted(seen),'assets':len(files),'source_private_receipt_binding':binding}))
    files[sums] = hashlib.sha256(checksums).hexdigest()
    return files
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--target',choices=TARGETS);p.add_argument('--output');p.add_argument('--all');p.add_argument('--version')
    p.add_argument('--receipt',type=pathlib.Path,default=pathlib.Path('candidate-verification.json'),
                   help='write the single-package verification receipt to this file')
    p.add_argument('--audit-receipt', type=pathlib.Path,
                   help='write the privacy report here instead of package-audit.json beside the candidate receipt')
    p.add_argument('--resolve-source-probe',type=pathlib.Path,
                   help='Resolve archive read-back tool built from the same source, required for Intel macOS')
    a=p.parse_args()
    if a.all:
        expected = gate_binding(os.environ['GITHUB_SHA'], os.environ['PRIVATE_GATE']) if os.environ.get('GITHUB_ACTIONS') == 'true' else None
        verify_all(a.all,a.version,expected_binding=expected)
    elif a.target and a.output:
        verify_package(a.target,a.output,a.version,receipt_path=a.receipt,
                       resolve_source_probe=a.resolve_source_probe, audit_receipt=a.audit_receipt)
    else:p.error('use --all or --target and --output')
