#!/usr/bin/env python3
"""Collect crate grants, attribution and required source without leaking cache paths."""
import argparse
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import tomllib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
NOTICE_PREFIXES = ('LICENSE', 'COPYING', 'COPYRIGHT', 'NOTICE', 'OFL', 'UFL', 'UNLICENSE', 'AUTHORS')

def applicable_grant(expression, texts):
    """An attribution filename alone cannot establish an applicable license grant."""
    text = ' '.join(re.sub(r'\s+', ' ', t).lower() for t in texts)
    found = {
        'MIT': 'permission is hereby granted' in text and 'without restriction' in text,
        'Apache-2.0': 'apache license' in text and 'grant of copyright license' in text,
        'BSD-2-Clause': 'redistribution and use in source and binary forms' in text,
        'BSD-3-Clause': 'redistribution and use in source and binary forms' in text,
        'ISC': 'permission to use, copy, modify, and/or distribute' in text,
        'Zlib': 'permission is granted to anyone to use this software' in text,
        'MPL-2.0': 'mozilla public license' in text and 'source code form' in text,
        'CC0-1.0': 'cc0' in text and 'waiver' in text,
        'Unicode-3.0': 'unicode' in text and 'permission is hereby granted' in text,
        'BSL-1.0': 'boost software license' in text and 'permission is hereby granted' in text,
        'Unlicense': 'unencumbered software released into the public domain' in text,
        '0BSD': 'permission to use, copy, modify, and/or distribute' in text,
        'OFL-1.1': 'sil open font license' in text and 'permission & conditions' in text,
        'Ubuntu-font-1.0': 'ubuntu font licence' in text and 'permission & conditions' in text,
    }
    # Cargo's historical slash spelling means OR. Respect AND obligations and
    # parentheses rather than accepting one grant anywhere in the expression.
    tokens = re.findall(r'\(|\)|[A-Za-z0-9.+-]+', expression.replace('/', ' OR '))
    index = 0
    def atom():
        nonlocal index
        if index >= len(tokens):
            raise ValueError('incomplete license expression')
        token = tokens[index]; index += 1
        if token == '(':
            value = either()
            if index >= len(tokens) or tokens[index] != ')':
                raise ValueError('unbalanced license expression')
            index += 1
        else:
            value = found.get(token, False)
        if index < len(tokens) and tokens[index] == 'WITH':
            index += 1
            if index >= len(tokens):
                raise ValueError('missing license exception')
            exception = tokens[index]; index += 1
            value = value and exception == 'LLVM-exception' and 'llvm exception' in text
        return value
    def both():
        nonlocal index
        value = atom()
        while index < len(tokens) and tokens[index] == 'AND':
            index += 1; other = atom(); value = value and other
        return value
    def either():
        nonlocal index
        value = both()
        while index < len(tokens) and tokens[index] == 'OR':
            index += 1; other = both(); value = value or other
        return value
    result = either()
    if index != len(tokens):
        raise ValueError('unsupported license expression')
    return result

def source_archive(package, base, expected, output):
    name = package['name'] + '-' + package['version'] + '.crate'
    url = f'https://static.crates.io/crates/{package["name"]}/{name}'
    target = output / 'sources' / 'rust' / name
    target.parent.mkdir(parents=True, exist_ok=True)
    cached = base.parents[2] / 'cache' / base.parent.name / name
    if cached.is_file():
        shutil.copy2(cached, target)
    else:
        urllib.request.urlretrieve(url, target)
    if not expected or hashlib.sha256(target.read_bytes()).hexdigest() != expected:
        raise ValueError('crate corresponding-source checksum mismatch: ' + package['name'])
    return {'file': target.relative_to(output).as_posix(), 'url': url, 'sha256': expected, 'modified': False}

def collect(output, source_root=ROOT):
    output = pathlib.Path(output)
    output.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1', '--manifest-path', str(source_root / 'crates/Cargo.toml')]))
    lock = tomllib.loads((source_root / 'crates/Cargo.lock').read_text())
    checksums = {(p['name'], p['version']): p.get('checksum') for p in lock['package']}
    supplements = ROOT / 'packaging/license-supplements'
    supplement_rows = json.loads((supplements / 'provenance.json').read_text())
    packages, sources = [], []
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        base = pathlib.Path(package['manifest_path']).parent
        expression = package['license'] or ('MIT' if package['source'] is None else None)
        if expression is None:
            raise ValueError('dependency has no license: ' + package['name'])
        item = {'name': package['name'], 'version': package['version'], 'license': expression, 'source': package['source'] or 'first-party', 'sha256': checksums.get((package['name'], package['version'])), 'authors': package['authors'], 'notices': []}
        grant_texts, all_notice_texts = [], []
        crate_expression = re.sub(r'\s+AND\s+(?:OFL-1\.1|Ubuntu-font-1\.0)', '', expression) if package['name'] == 'epaint_default_fonts' else expression
        if package['source'] is not None:
            for path in sorted(base.rglob('*')):
                if not path.is_file():
                    continue
                relative = path.relative_to(base)
                is_notice = path.name.upper().startswith(NOTICE_PREFIXES) or any(p.upper() in ('LICENSES', 'LICENCES') for p in relative.parts[:-1]) or (package['name'] == 'epaint_default_fonts' and path.suffix.lower() == '.txt')
                if not is_notice:
                    continue
                target = output / 'licenses/rust' / (package['name'] + '-' + package['version']) / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, target)
                item['notices'].append(target.relative_to(output).as_posix())
                # Font/third-party subdirectories retain their own attribution,
                # but cannot stand in for the crate's own license grant.
                all_notice_texts.append(path.read_text(errors='replace'))
                if len(relative.parts) == 1 or relative.parts[0].upper() in ('LICENSES', 'LICENCES') or package.get('license_file') == str(path):
                    grant_texts.append(path.read_text(errors='replace'))
            if not applicable_grant(crate_expression, grant_texts):
                selected = [r for r in supplement_rows if r['package'] == package['name'] + '-' + package['version']]
                if not selected:
                    raise ValueError('dependency has no applicable crate grant: ' + package['name'])
                for row in selected:
                    path = supplements / row['file']
                    if hashlib.sha256(path.read_bytes()).hexdigest() != row['sha256']:
                        raise ValueError('license supplement hash mismatch: ' + row['file'])
                    target = output / 'licenses/rust' / (package['name'] + '-' + package['version']) / row['file']
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
                    item['notices'].append(target.relative_to(output).as_posix())
                    grant_texts.append(path.read_text(errors='replace'))
                    all_notice_texts.append(path.read_text(errors='replace'))
                item['notice_provenance'] = selected
                if package['repository'] == 'https://github.com/madsmtm/objc2' or any(r.get('supply_mit_grant') for r in selected):
                    if 'MIT' not in set(re.findall(r'[A-Za-z0-9.+-]+', expression)):
                        raise ValueError('MIT supplement does not apply to crate')
                    target = target.parent / 'MIT.txt'
                    grant = (ROOT / 'LICENSE').read_text().split('Permission is hereby', 1)[1]
                    text = 'MIT License\n\nUpstream crate authors: ' + ', '.join(package['authors']) + '\n\nPermission is hereby' + grant
                    target.write_text(text)
                    item['notices'].append(target.relative_to(output).as_posix())
                    grant_texts.append(text)
                    all_notice_texts.append(text)
                if not applicable_grant(crate_expression, grant_texts):
                    raise ValueError('supplements do not establish applicable crate grant: ' + package['name'])
            if not applicable_grant(expression, all_notice_texts):
                raise ValueError('dependency license obligations lack grants: ' + package['name'])
            if 'MPL-2.0' in expression:
                item['corresponding_source'] = source_archive(package, base, item['sha256'], output)
                sources.append(item['corresponding_source'])
        packages.append(item)
    for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'avid-reader/NOTICE', 'avid-reader/LICENSE-pyavb', 'avid-reader/LICENSE-filmcraft', 'vendor/py_premiere/LICENSE', 'vendor/FORK.md'):
        target = output / 'licenses' / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, target)
    (output / 'SOURCE_AVAILABILITY.md').write_text('# Corresponding source\n\nFFmpeg corresponding source and its build recipe accompany the media tools.\n\nRust crates declaring MPL 2.0 are supplied unmodified in their exact registry\nsource archives below. Each SHA256 matches Cargo.lock. Extract the .crate file\nwith a tar reader to inspect, modify or rebuild the covered source. The crate\nlicense and upstream attribution remain in licenses/rust/.\n\n' + ''.join(f'- `{s["file"]}` — SHA256 `{s["sha256"]}`; upstream [{s["url"]}]({s["url"]}).\n' for s in sources))
    (output / 'sbom.json').write_text(json.dumps({'schema': 'anynle.sbom/1', 'scope': 'Cargo.lock dependency superset; per-platform binaries may link fewer packages', 'packages': packages}, indent=2) + '\n')

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('output')
    parser.add_argument('--source-root', type=pathlib.Path, default=ROOT)
    args = parser.parse_args()
    collect(args.output, args.source_root)
