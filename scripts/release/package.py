#!/usr/bin/env python3
"""Package native Rust GUI+CLI with verified adjacent media tools."""
import argparse, hashlib, json, os, pathlib, plistlib, shutil, subprocess, sys, tarfile
from build_ffmpeg import fetch, sha
from appimage_runtime import bundle as appimage_runtime
from windows_pe import verify as verify_windows_pe
from release_version import release_version
ROOT=pathlib.Path(__file__).resolve().parents[2]
ICONS=ROOT/'packaging/icons'
TARGETS={
'aarch64-apple-darwin':('macOS_ARM','.zip'), 'x86_64-apple-darwin':('macOS_x64','.zip'),
'x86_64-pc-windows-msvc':('Windows_x64','.exe'), 'aarch64-pc-windows-msvc':('Windows_ARM','.exe'),
'x86_64-unknown-linux-gnu':('Linux_x64','.AppImage'), 'aarch64-unknown-linux-gnu':('Linux_ARM','.AppImage')}
def verify_backend(directory,target):
    directory=pathlib.Path(directory)
    m=json.loads((directory/'backend-manifest.json').read_text())
    spec=json.loads((ROOT/'packaging/ffmpeg.json').read_text())
    if m['schema']!=1 or m['target']!=target or m['ffmpeg_version']!=spec['version'] or m['license']!=spec['license'] or m['source']!={'url':spec['url'],'sha256':spec['sha256']}: raise ValueError('backend provenance mismatch')
    if any(f in m['configure'] for f in ['--enable-gpl','--enable-version3','--enable-nonfree']): raise ValueError('forbidden backend configuration')
    if not set(spec['configure']).issubset(m['configure']): raise ValueError('required backend configuration absent')
    for name in ('ffmpeg','ffprobe'):
        expected=name+('.exe' if 'windows' in target else '')
        if m['tools'][name]['file']!=expected: raise ValueError('backend filename mismatch')
        p=directory/expected
        if p.is_symlink() or sha(p)!=m['tools'][name]['sha256']: raise ValueError('backend checksum mismatch')
    if sha(directory/'ffmpeg-8.1-source.tar.xz')!=spec['sha256']: raise ValueError('corresponding source absent or wrong')
    return m

def assert_native_dependencies(binary,target):
    if 'apple' in target:
        lines=subprocess.check_output(['otool','-L',str(binary)],text=True).splitlines()[1:]
        for line in lines:
            dep=line.strip().split(' (')[0]
            if not dep.startswith(('/usr/lib/','/System/Library/')): raise ValueError('non-system macOS runtime dependency: '+dep)
    elif 'windows' in target:
        return verify_windows_pe(binary)

def linux_dependencies(stage):
    # Distro libraries are host prerequisites. Copying them would require their
    # own notices and (where applicable) corresponding-source handling.
    report={'schema':'anynle.linux-runtime/1','policy':'host system libraries; no distro shared objects redistributed','baseline':'Ubuntu 22.04 or compatible glibc 2.35 desktop','binaries':{}}
    for binary in (stage/'usr/bin').iterdir():
        if binary.name not in ('any-nle','any-nle-gui','ffmpeg','ffprobe'):
            continue
        result=subprocess.run(['ldd',str(binary)],text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        out=result.stdout
        if result.returncode or 'not found' in out:
            raise ValueError('missing or unreadable Linux host runtime dependency')
        names=[]
        for line in out.splitlines():
            parts=line.strip().split()
            if len(parts)>=3 and parts[1]=='=>' and parts[2].startswith('/'):
                names.append(parts[0])
            elif parts and parts[0].startswith('/'):
                names.append(pathlib.Path(parts[0]).name)
        report['binaries'][binary.name]=sorted(set(names))
    resources=stage/'usr/share/any-nle'
    (resources/'linux-runtime.json').write_text(json.dumps(report,indent=2)+'\n')
    shutil.copy2(ROOT/'packaging/LINUX_RUNTIME_REQUIREMENTS.md',resources/'LINUX_RUNTIME_REQUIREMENTS.md')

def package(target,binaries,backend,notices,output,version):
    version = release_version(version)
    verify_backend(backend,target)
    binaries,backend,notices,output=map(pathlib.Path,(binaries,backend,notices,output))
    output.mkdir(parents=True,exist_ok=True)
    suffix,extension=TARGETS[target]; stem=f'Any.NLE_{suffix}_v{version}'
    stage=output/(stem+'.stage')
    if stage.exists(): raise ValueError('refusing to replace an existing package stage')
    if 'apple' in target: bindir=stage/'Any NLE.app/Contents/MacOS'; resources=stage/'Any NLE.app/Contents/Resources'
    elif 'linux' in target: bindir=stage/'usr/bin'; resources=stage/'usr/share/any-nle'
    else: bindir=stage; resources=stage/'resources'
    bindir.mkdir(parents=True,exist_ok=True); resources.mkdir(parents=True,exist_ok=True)
    exe='.exe' if 'windows' in target else ''
    for name in ('any-nle','any-nle-gui'):
        shutil.copy2(binaries/(name+exe),bindir/(name+exe))
    for name in ('ffmpeg','ffprobe','backend-manifest.json'):
        filename=name+exe if name!='backend-manifest.json' else name
        shutil.copy2(backend/filename,bindir/filename)
    # Ad hoc signing changes Mach-O bytes: sign backend tools first, then pin the
    # actual shipped bytes, then sign the app after creating its final resources.
    if 'apple' in target:
        for name in ('ffmpeg','ffprobe','any-nle','any-nle-gui'):
            assert_native_dependencies(bindir/name,target)
            subprocess.run(['codesign','--force','--sign','-',str(bindir/name)],check=True)
        manifest=json.loads((bindir/'backend-manifest.json').read_text())
        for name in ('ffmpeg','ffprobe'): manifest['tools'][name]['sha256']=sha(bindir/name)
        (bindir/'backend-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    else:
        for name in ('any-nle','any-nle-gui','ffmpeg','ffprobe'): assert_native_dependencies(bindir/(name+exe),target)
    if 'windows' in target:
        copyright=ROOT/'packaging/license-supplements/nsis-3.11-COPYING'
        provenance=ROOT/'packaging/license-supplements/nsis-3.11-provenance.json'
        if sha(copyright)!=json.loads(provenance.read_text())['sha256']:raise ValueError('NSIS notice checksum mismatch')
        nsis_notices=resources/'licenses/nsis';nsis_notices.mkdir(parents=True,exist_ok=True)
        shutil.copy2(copyright,nsis_notices/'COPYING');shutil.copy2(provenance,nsis_notices/'provenance.json')
    for src in (notices,backend):
        for p in src.iterdir():
            if p.name in ('ffmpeg','ffprobe','ffmpeg.exe','ffprobe.exe','backend-manifest.json'): continue
            dest=resources/p.name
            if p.is_dir(): shutil.copytree(p,dest,dirs_exist_ok=True)
            else: shutil.copy2(p,dest)
    shutil.copy2(ROOT/'LICENSE',resources/'LICENSE'); shutil.copy2(ROOT/'THIRD_PARTY_NOTICES.md',resources/'THIRD_PARTY_NOTICES.md')
    if 'apple' in target:
        numeric_version = version.split('-', 1)[0]
        contents=stage/'Any NLE.app/Contents'
        info={'CFBundleName':'Any NLE','CFBundleDisplayName':'Any NLE','CFBundleIdentifier':'industries.whatever.any-nle','CFBundleExecutable':'any-nle-gui','CFBundlePackageType':'APPL','CFBundleShortVersionString':numeric_version,'CFBundleVersion':numeric_version,'LSMinimumSystemVersion':'12.0','NSHighResolutionCapable':True}
        info['CFBundleIconFile']='any-nle.icns'
        shutil.copy2(ICONS/'any-nle.icns',resources/info['CFBundleIconFile'])
        (contents/'Info.plist').write_bytes(plistlib.dumps(info))
        subprocess.run(['codesign','--force','--deep','--sign','-',str(stage/'Any NLE.app')],check=True)
        subprocess.run(['codesign','--verify','--deep','--strict',str(stage/'Any NLE.app')],check=True)
        # Signing the containing bundle must not invalidate backend checksum pins.
        for name in ('ffmpeg','ffprobe'):
            if sha(bindir/name)!=manifest['tools'][name]['sha256']: raise ValueError('backend bytes changed after signing')
        subprocess.run(['ditto','-c','-k','--sequesterRsrc','--keepParent',str(stage/'Any NLE.app'),str(output/(stem+extension))],check=True)
    elif 'windows' in target:
        resource_files=sorted((p for p in resources.rglob('*') if p.is_file()),key=lambda p:str(p))
        resource_dirs=sorted((p for p in resources.rglob('*') if p.is_dir()),key=lambda p:len(p.parts),reverse=True)
        uninstall='\n'.join('  Delete "$INSTDIR\\'+str(p.relative_to(stage)).replace('/','\\')+'"' for p in resource_files)
        uninstall+='\n'+'\n'.join('  RMDir "$INSTDIR\\'+str(p.relative_to(stage)).replace('/','\\')+'"' for p in resource_dirs+[resources])
        script=(ROOT/'packaging/windows.nsi').read_text().replace('@VERSION@',version).replace('@SOURCE@',str(stage.resolve())).replace('@OUTPUT@',str((output/(stem+extension)).resolve())).replace('@ICON@',str(ICONS/'any-nle.ico')).replace('@UNINSTALL_RESOURCES@',uninstall)
        nsifile=output/(stem+'.nsi'); nsifile.write_text(script)
        makensis=shutil.which('makensis') or r'C:\Program Files (x86)\NSIS\makensis.exe'
        version_output=subprocess.check_output([makensis,'/VERSION'],text=True).strip()
        if version_output!='v3.11':raise ValueError('reviewed NSIS 3.11 is required')
        subprocess.run([makensis,str(nsifile)],check=True)
        verify_windows_pe(output/(stem+extension))
    else:
        linux_dependencies(stage)
        (stage/'AppRun').write_text('#!/bin/sh\nHERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$HERE/usr/bin/any-nle-gui" "$@"\n')
        (stage/'AppRun').chmod(0o755)
        (stage/'any-nle.desktop').write_text('[Desktop Entry]\nType=Application\nName=Any NLE\nExec=any-nle-gui\nIcon=any-nle\nCategories=AudioVideo;\nTerminal=false\n')
        shutil.copy2(ICONS/'any-nle.png',stage/'any-nle.png')
        arch='aarch64' if target.startswith('aarch64') else 'x86_64'
        spec=json.loads((ROOT/'packaging/ffmpeg.json').read_text())['appimagetool']
        tool=output/('appimagetool-'+arch+'.AppImage')
        fetch(f'https://github.com/AppImage/appimagetool/releases/download/{spec["version"]}/appimagetool-{arch}.AppImage',tool,spec[arch]); tool.chmod(0o755)
        runtime=appimage_runtime(arch,resources,output/'appimage-runtime-cache')
        subprocess.run([str(tool.resolve()),'--appimage-extract-and-run','--runtime-file',str(runtime.resolve()),str(stage.resolve()),str((output/(stem+extension)).resolve())],env=dict(os.environ,ARCH=arch),check=True)
    provenance=output/(stem+'.provenance.tar.gz')
    with tarfile.open(provenance,'w:gz') as t:
        t.add(resources,arcname='resources'); t.add(bindir/'backend-manifest.json',arcname='backend-manifest.json')
        t.add(ROOT/'packaging/ffmpeg.json',arcname='packaging/ffmpeg.json')
        t.add(ROOT/'crates/gui/Cargo.toml',arcname='crates/gui/Cargo.toml')
        t.add(ROOT/'packaging/appimage-runtime.json',arcname='packaging/appimage-runtime.json')
        for script in ('build_ffmpeg.py','package.py','licenses.py','appimage_runtime.py','windows_pe.py','release_version.py'): t.add(ROOT/'scripts/release'/script,arcname='scripts/release/'+script)
    for p in (output/(stem+extension),provenance):
        (output/(p.name+'.sha256')).write_text(sha(p)+'  '+p.name+'\n')
    print(json.dumps({'package':stem+extension,'sha256':sha(output/(stem+extension))}))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--target',required=True,choices=TARGETS);p.add_argument('--binaries',required=True);p.add_argument('--backend',required=True);p.add_argument('--notices',required=True);p.add_argument('--output',required=True);p.add_argument('--version',default=None);a=p.parse_args();package(a.target,a.binaries,a.backend,a.notices,a.output,a.version)
