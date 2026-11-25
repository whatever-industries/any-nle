import gzip,io,json,pathlib,sys,tempfile,unittest,zipfile,zlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
from audit import Scanner, audit
from package import RESOLVE_TEMPLATES, verify_backend, verify_resolve_templates
from publish import validate_existing

class ResolveTemplateTests(unittest.TestCase):
    def test_bundle_requires_exact_reviewed_template_bytes(self):
        import shutil
        with tempfile.TemporaryDirectory() as tmp:
            directory=pathlib.Path(tmp)/'resolve-templates'
            shutil.copytree(RESOLVE_TEMPLATES,directory)
            verify_resolve_templates(directory)
            donor=directory/'Group.drp'
            donor.write_bytes(donor.read_bytes()+b'\0')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                verify_resolve_templates(directory)
            donor.unlink()
            with self.assertRaisesRegex(ValueError, 'inventory mismatch'):
                verify_resolve_templates(directory)

class AuditTests(unittest.TestCase):
    def test_generic_private_sentinel_in_nested_compressed_utf16_is_found(self):
        nested=zlib.compress('SyntheticPrivateSentinel'.encode('utf-16-le'))
        buf=io.BytesIO()
        with zipfile.ZipFile(buf,'w',zipfile.ZIP_DEFLATED) as z:z.writestr('generic.prproj',gzip.compress(b'header'+nested))
        scanner=Scanner(['SyntheticPrivateSentinel']);scanner.scan(buf.getvalue())
        self.assertFalse(scanner.report()['passed'])
        self.assertGreater(scanner.objects,3)
        self.assertNotIn('SyntheticPrivateSentinel',json.dumps(scanner.report()))
    def test_credentials_in_archive_are_found(self):
        scanner=Scanner();scanner.scan(gzip.compress(b'ghp_'+b'a'*36));self.assertFalse(scanner.report()['passed'])
    def test_private_key_payload_required_not_just_parser_marker(self):
        scanner=Scanner();scanner.scan(b'parser recognizes -----BEGIN PRIVATE KEY-----');self.assertTrue(scanner.report()['passed'])
        scanner.scan(b'-----BEGIN PRIVATE KEY-----\n'+b'YWJjZGVm'*8+b'\n-----END PRIVATE KEY-----')
        self.assertFalse(scanner.report()['passed'])
    def test_malformed_gzip_records_incomplete_failed_gate(self):
        scanner=Scanner();scanner.scan(b'\x1f\x8b\x08\x00'+b'bad gzip fixture')
        report=scanner.report();self.assertFalse(report['passed']);self.assertEqual(len(report['undecodable_archives']),1)
    def test_only_exact_known_upstream_malformed_fixture_is_accepted(self):
        import hashlib
        data=b'\x1f\x8b\x08\x00'+b'generic malformed test fixture'
        known=[{'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)}]
        scanner=Scanner(known_upstream=known);scanner.scan(data);self.assertTrue(scanner.report()['passed']);self.assertFalse(scanner.report()['expansion_complete'])
        scanner=Scanner(known_upstream=known);scanner.scan(data+b'modified');self.assertFalse(scanner.report()['passed'])
        scanner=Scanner(['malformed'],known_upstream=known);scanner.scan(data);self.assertFalse(scanner.report()['passed'])
    def test_clean_archive_passes(self):
        scanner=Scanner(['SyntheticPrivateSentinel']);scanner.scan(gzip.compress(b'generic synthetic test data'));self.assertTrue(scanner.report()['passed'])
    def test_expansion_budget_refuses(self):
        scanner=Scanner(budget=1000)
        with self.assertRaises(ValueError):scanner.scan(gzip.compress(b'x'*5000))

    def test_unavailable_audit_input_refuses_even_with_a_clean_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            (root / 'clean.txt').write_bytes(b'synthetic contents')
            self.assertTrue(audit([root])['passed'])
            with self.assertRaisesRegex(ValueError, 'audit input is unavailable'):
                audit([root, root / 'missing.tar.gz'])

    def test_relocated_extraction_has_the_same_redacted_audit_locations(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            first = root / 'extracted-one'
            second = root / 'extracted-two'
            for extracted in (first, second):
                extracted.mkdir()
                (extracted / 'extra.txt').write_text('SyntheticPrivateSentinel')
            one = audit([first], ['SyntheticPrivateSentinel'])
            two = audit([second], ['SyntheticPrivateSentinel'])
            self.assertFalse(one['passed'])
            self.assertEqual(one, two)
            self.assertNotIn('extracted-one', json.dumps(one))

class ReleaseVersionTests(unittest.TestCase):
    def test_cargo_version_controls_early_workflow_validation(self):
        from release_version import validate_workflow
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            manifest = root / 'crates/gui/Cargo.toml'
            manifest.parent.mkdir(parents=True)
            manifest.write_text('[package]\nversion = "2.1.7-alpha.4"\n')
            notes = root / 'RELEASE_NOTES.md'
            notes.write_text('# Any NLE v2.1.7-alpha.4\nhttps://example.invalid/releases/download/v2.1.7-alpha.4/file\n')
            source, receipt = 'a' * 40, 'b' * 64
            with patch('release_version.subprocess.check_output', side_effect=[source, b'src/main.rs\0']):
                self.assertIn(source, validate_workflow('2.1.7-alpha.4', source, receipt, source, root=root))
            for version, reviewed, private, event, error in [
                ('2.1.7-alpha.3', source, receipt, source, 'Cargo version'),
                ('2.1.7-alpha.4', 'bad', receipt, source, 'export SHA'),
                ('2.1.7-alpha.4', source, 'bad', source, 'receipt digest'),
                ('2.1.7-alpha.4', source, receipt, 'c' * 40, 'checkout'),
            ]:
                with self.subTest(error=error), patch('release_version.subprocess.check_output', return_value=source):
                    with self.assertRaisesRegex(ValueError, error):
                        validate_workflow(version, reviewed, private, event, root=root)
            notes.write_text(notes.read_text().replace('/v2.1.7-alpha.4/', '/v2.1.7-alpha.3/'))
            with self.assertRaisesRegex(ValueError, 'download links'):
                validate_workflow('2.1.7-alpha.4', source, receipt, source, root=root)

    def test_aggregate_refuses_wrong_version_and_unbound_candidates(self):
        from intel_resolve_smoke import expected_receipt
        from package import TARGETS
        from release_version import gate_binding, release_version
        from verify_candidate import INTEL_MAC, verify_all
        from build_ffmpeg import sha

        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            version = release_version()
            binding = gate_binding('a' * 40, 'b' * 64)
            for target, (suffix, ext) in TARGETS.items():
                candidate = root / ('any-nle-' + suffix)
                candidate.mkdir()
                package = candidate / f'Any.NLE_{suffix}_v{version}{ext}'
                package.write_bytes(b'synthetic package')
                provenance = package.with_suffix('.provenance.tar.gz')
                provenance.write_bytes(b'synthetic provenance')
                for file in (package, provenance):
                    (candidate / (file.name + '.sha256')).write_text(f'{sha(file)}  {file.name}\n')
                audit_path = candidate / 'package-audit.json'
                audit_path.write_text(json.dumps(dict(schema='anynle.privacy-audit/1', passed=True,
                                                      match_locations=[], undecodable_archives=[])))
                receipt = dict(target=target, version=version, package=package.name, sha256=sha(package),
                               provenance_sha256=sha(provenance),
                               package_audit_sha256=sha(audit_path),
                               extracted_bytes_match=True, architecture_verified=True, launcher_icon_verified=True,
                               cli_smoke=True, backend_smoke=True,
                               backend_media=dict(schema='anynle.backend-smoke/1', target=target,
                                                  passed=True, synthetic_cases=[
                                   dict(codec='dnxhd', container='mxf', decoded_picture_and_pcm_equal=True),
                                   dict(codec='prores_ks', container='mov', decoded_picture_and_pcm_equal=True),
                                   dict(codec='png', container='png', decoded_rgba_equal=True),
                               ]))
                if target == INTEL_MAC:
                    receipt['avid_resolve_smoke'] = expected_receipt()
                for name, value in [
                    ('candidate-verification', receipt),
                    ('public-acceptance', dict(profile='synthetic-only', passed=118, private_corpus_qualification=False)),
                ]:
                    (candidate / (name + '.json')).write_text(json.dumps(value))
                (candidate / 'gate-binding.txt').write_text(binding)
            from unittest.mock import patch
            with patch('verify_candidate.sha', wraps=sha) as hash_file:
                verified = verify_all(root, version, expected_binding=binding)
            self.assertEqual(len(verified), 13)
            sums = root / 'SHA256SUMS'
            self.assertCountEqual([call.args[0] for call in hash_file.call_args_list],
                                  [path for path in verified if path != sums])
            self.assertEqual(verified[sums], sha(sums))
            self.assertEqual(sums.read_text().splitlines(), [
                f'{digest}  {path.name}' for path, digest in
                sorted(verified.items(), key=lambda row: row[0].name) if path != sums
            ])
            arm_candidate = root / 'any-nle-macOS_ARM'
            arm_receipt = arm_candidate / 'candidate-verification.json'
            verified_arm = json.loads(arm_receipt.read_text())
            backend_media = verified_arm['backend_media']
            # A smoke receipt from the build directory does not qualify the package.
            (arm_candidate / 'backend-smoke.json').write_text(json.dumps(backend_media))
            for bad_media in (
                None,
                dict(backend_media, synthetic_cases=backend_media['synthetic_cases'][:-1]),
                dict(backend_media, target=INTEL_MAC),
            ):
                unverified_arm = dict(verified_arm)
                if bad_media is None:
                    del unverified_arm['backend_media']
                else:
                    unverified_arm['backend_media'] = bad_media
                arm_receipt.write_text(json.dumps(unverified_arm))
                with self.assertRaisesRegex(ValueError, 'extracted synthetic backend smoke incomplete'):
                    verify_all(root, version)
            arm_receipt.write_text(json.dumps(verified_arm))
            arm_audit = arm_candidate / 'package-audit.json'
            original_audit = arm_audit.read_bytes()
            altered_audit = dict(json.loads(original_audit), objects=999)
            arm_audit.write_text(json.dumps(altered_audit))
            with self.assertRaisesRegex(ValueError, 'verification does not bind privacy audit'):
                verify_all(root, version)
            altered_audit['passed'] = False
            arm_audit.write_text(json.dumps(altered_audit))
            arm_receipt.write_text(json.dumps(dict(verified_arm, package_audit_sha256=sha(arm_audit))))
            with self.assertRaisesRegex(ValueError, 'package privacy gate failed'):
                verify_all(root, version)
            arm_audit.write_bytes(original_audit)
            arm_receipt.write_text(json.dumps(verified_arm))
            arm_provenance = next(arm_candidate.glob('*.provenance.tar.gz'))
            original_provenance = arm_provenance.read_bytes()
            proof = arm_provenance.with_name(arm_provenance.name + '.sha256')
            original_proof = proof.read_text()
            arm_provenance.write_bytes(b'different provenance')
            proof.write_text(f'{sha(arm_provenance)}  {arm_provenance.name}\n')
            with self.assertRaisesRegex(ValueError, 'verification does not bind provenance'):
                verify_all(root, version)
            arm_provenance.write_bytes(original_provenance)
            proof.write_text(original_proof)
            intel_receipt = root / 'any-nle-macOS_x64' / 'candidate-verification.json'
            verified_intel = json.loads(intel_receipt.read_text())
            for smoke in (None, dict(expected_receipt(), pcm_channels=1)):
                invalid_intel = dict(verified_intel, avid_resolve_smoke=smoke)
                intel_receipt.write_text(json.dumps(invalid_intel))
                with self.assertRaisesRegex(ValueError, 'Intel Avid-to-Resolve package smoke'):
                    verify_all(root, version)
            intel_receipt.write_text(json.dumps(verified_intel))
            with self.assertRaisesRegex(ValueError, 'Cargo version'):
                verify_all(root, '9.9.9')
            with self.assertRaisesRegex(ValueError, 'reviewed source'):
                verify_all(root, version, expected_binding=gate_binding('c' * 40, 'b' * 64))
            (candidate / 'gate-binding.txt').write_text(gate_binding('c' * 40, 'b' * 64))
            with self.assertRaisesRegex(ValueError, 'bindings differ'):
                verify_all(root, version)
            (candidate / 'gate-binding.txt').write_text(binding)
            receipt['version'] = '9.9.9'
            (candidate / 'candidate-verification.json').write_text(json.dumps(receipt))
            with self.assertRaisesRegex(ValueError, 'verification does not bind'):
                verify_all(root, version)


class PublicationTests(unittest.TestCase):
    def test_published_version_and_wrong_source_cannot_be_reused(self):
        validate_existing([], 'a'*40)
        validate_existing([{'draft':True,'target_commitish':'a'*40}], 'a'*40)
        for r in ({'draft':False,'target_commitish':'a'*40},{'draft':True,'target_commitish':'b'*40}):
            with self.assertRaises(ValueError):validate_existing([r], 'a'*40)

class DraftRecoveryTests(unittest.TestCase):
    def setUp(self):
        from subprocess import CalledProcessError
        from unittest.mock import patch
        import os
        import publish
        from release_version import release_version

        self.release = publish
        self.failure = CalledProcessError
        self.version = release_version()
        self.tag = 'v' + self.version
        self.head = 'a' * 40
        self.repo = 'example/any-nle'
        self.remote = {}
        self.draft = None
        self.draft_creations = 0
        self.hidden_draft_reads = 0
        self.refs = []
        self.uploads = []
        self.fail_upload = None
        self.replace_after_upload = False
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = pathlib.Path(temp.name)
        (self.root / 'RELEASE_NOTES.md').write_text(
            '# Any NLE\n\nPremiere → Resolve\n', encoding='utf-8',
        )
        self.files = [self.root / name for name in ('package.zip', 'provenance.tar.gz')]
        for p in self.files:
            p.write_text(p.name)
        self.wait = self.enterContext(patch('publish.sleep'))
        for replacement in [
            patch('publish.gh', side_effect=self.github),
            patch('publish.ROOT', self.root),
            patch('publish.verify_all', side_effect=lambda *_args, **_kwargs: {
                path: publish.sha(path) for path in self.files
            }),
            patch('publish.validate_workflow', return_value='reviewed binding'),
            patch.dict(os.environ, GITHUB_REPOSITORY=self.repo,
                       GITHUB_SHA=self.head, PRIVATE_GATE='b' * 64),
        ]:
            replacement.start()
            self.addCleanup(replacement.stop)

    def github(self, *args):
        from hashlib import sha256
        from urllib.parse import parse_qs, urlsplit

        api = f'repos/{self.repo}'
        if args == ('api', '--paginate', api + '/releases?per_page=100', '--slurp'):
            if self.draft and self.hidden_draft_reads:
                self.hidden_draft_reads -= 1
                return '[[]]'
            return json.dumps([[self.draft] if self.draft else []])
        if args == ('api', f'{api}/git/matching-refs/tags/{self.tag}'):
            return json.dumps(self.refs)
        if args == ('api', api):
            return '{"private":false}'
        if args[:2] == ('release', 'create'):
            self.assertIsNone(self.draft)
            self.assertIn('--draft', args)
            self.assertIn('--prerelease', args)
            self.assertEqual(args[args.index('--target') + 1], self.head)
            self.draft_creations += 1
            notes = pathlib.Path(args[args.index('--notes-file') + 1])
            self.draft = dict(id=7, tag_name=self.tag, draft=True, prerelease=True,
                              target_commitish=self.head,
                              body=notes.read_text(encoding='utf-8').strip(),
                              upload_url=f'https://uploads.github.com/{api}/releases/7/assets' + '{?name,label}')
            return ''
        if args == ('api', '--paginate', f'{api}/releases/7/assets?per_page=100', '--slurp'):
            return json.dumps([[dict(name=name, state='uploaded', digest='sha256:' + sha256(data).hexdigest())
                                for name, data in self.remote.items()]])
        if args[:3] == ('api', '--method', 'POST'):
            url = urlsplit(args[3])
            self.assertEqual(url.netloc, 'uploads.github.com')
            self.assertEqual(url.path, f'/{api}/releases/7/assets')
            name, = parse_qs(url.query)['name']
            self.assertNotIn(name, self.remote)
            if name == self.fail_upload:
                raise self.failure(1, args)
            self.uploads.append(name)
            self.remote[name] = pathlib.Path(args[args.index('--input') + 1]).read_bytes()
            if self.replace_after_upload:
                self.draft['id'] += 1
            return ''
        if args[:2] == ('release', 'download'):
            dest = pathlib.Path(args[args.index('--dir') + 1])
            for name, data in self.remote.items():
                (dest / name).write_bytes(data)
            return ''
        if args[:4] == ('api', '--method', 'PATCH', f'{api}/releases/7'):
            self.assertEqual(args[4:], ('-F', 'draft=false', '-f', 'make_latest=false'))
            self.draft['draft'] = False
            return json.dumps(self.draft)
        self.fail(f'unexpected GitHub command: {args}')

    def test_new_draft_can_take_time_to_appear(self):
        from unittest.mock import call

        self.hidden_draft_reads = 2
        self.release.publish(self.root, self.version, True)
        self.assertEqual(self.wait.call_args_list, [call(1), call(2)])
        self.assertEqual(self.draft_creations, 1)
        self.assertEqual(self.uploads, [p.name for p in self.files])
        self.assertFalse(self.draft['draft'])

    def test_changed_bytes_after_verification_cannot_stage_a_draft(self):
        from unittest.mock import patch

        verified = {path: self.release.sha(path) for path in self.files}
        self.files[0].write_bytes(b'replaced after verification')
        with patch('publish.verify_all', return_value=verified), patch('publish.gh') as gh:
            with self.assertRaisesRegex(ValueError, 'candidate changed after verification'):
                self.release.publish(self.root, self.version, True)
            gh.assert_not_called()

    def test_invisible_draft_can_recover_without_creating_another(self):
        from unittest.mock import call

        self.hidden_draft_reads = 5
        with self.assertRaisesRegex(ValueError, 'resume with the original reviewed candidates'):
            self.release.publish(self.root, self.version, True)
        self.assertEqual(self.wait.call_args_list, [call(1), call(2), call(4), call(8)])
        self.assertEqual(self.uploads, [])
        self.assertTrue(self.draft['draft'])
        self.release.publish(self.root, self.version, True)
        self.assertEqual(self.draft_creations, 1)
        self.assertEqual(self.uploads, [p.name for p in self.files])
        self.assertFalse(self.draft['draft'])

    def test_conflicting_draft_stops_during_visibility_retry(self):
        self.hidden_draft_reads = 1

        def replace_source(_):
            self.draft['target_commitish'] = 'c' * 40

        self.wait.side_effect = replace_source
        with self.assertRaisesRegex(ValueError, 'different source revision'):
            self.release.publish(self.root, self.version, True)
        self.wait.assert_called_once_with(1)
        self.assertEqual(self.uploads, [])

    def test_interrupted_upload_resumes_then_identical_retry_publishes(self):
        self.fail_upload = self.files[1].name
        with self.assertRaises(self.failure):
            self.release.publish(self.root, self.version, True)
        self.assertTrue(self.draft['draft'])
        self.assertEqual(self.uploads, [self.files[0].name])
        self.fail_upload = None
        self.files[1].write_text('different rebuild of missing asset')
        with self.assertRaisesRegex(ValueError, 'does not match the reviewed notes and asset set'):
            self.release.publish(self.root, self.version, False)
        self.assertEqual(self.uploads, [self.files[0].name])
        self.files[1].write_text(self.files[1].name)
        self.release.publish(self.root, self.version, False)
        self.assertEqual(self.uploads, [p.name for p in self.files])
        self.release.publish(self.root, self.version, True)
        self.assertEqual(self.uploads, [p.name for p in self.files])
        with self.assertRaisesRegex(ValueError, 'immutable'):
            self.release.publish(self.root, self.version, True)

    def test_conflicting_attempts_preserve_draft_and_existing_assets(self):
        self.release.publish(self.root, self.version, False)
        self.remote[self.files[0].name] = b'changed on GitHub'
        with self.assertRaisesRegex(ValueError, 'asset differs'):
            self.release.publish(self.root, self.version, True)
        self.remote[self.files[0].name] = self.files[0].read_bytes()
        self.remote['surprise'] = b'unreviewed'
        with self.assertRaisesRegex(ValueError, 'unexpected'):
            self.release.publish(self.root, self.version, True)
        self.refs = [{'ref': 'refs/tags/' + self.tag}]
        with self.assertRaisesRegex(ValueError, 'tag already exists'):
            self.release.publish(self.root, self.version, True)
        self.refs.clear()
        self.draft['target_commitish'] = 'c' * 40
        with self.assertRaisesRegex(ValueError, 'different source'):
            self.release.publish(self.root, self.version, True)
        self.assertEqual(self.uploads, [p.name for p in self.files])
        self.assertTrue(self.draft['draft'])

    def test_draft_replacement_stops_before_next_upload(self):
        self.replace_after_upload = True
        with self.assertRaisesRegex(ValueError, 'draft changed'):
            self.release.publish(self.root, self.version, True)
        self.assertEqual(self.uploads, [self.files[0].name])
        self.assertTrue(self.draft['draft'])


class BackendTests(unittest.TestCase):
    def test_missing_backend_refuses(self):
        with tempfile.TemporaryDirectory() as t:
            with self.assertRaises(FileNotFoundError):verify_backend(pathlib.Path(t),'aarch64-apple-darwin')

    def test_extracted_backend_uses_its_packaged_source_archive(self):
        from build_ffmpeg import sha
        from unittest.mock import patch
        import package

        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            bindir = root / 'bin'
            resources = root / 'resources'
            config = root / 'packaging'
            for path in (bindir, resources, config):
                path.mkdir()
            spec = json.loads((package.ROOT / 'packaging/ffmpeg.json').read_text())
            archive = resources / 'ffmpeg-8.1-source.tar.xz'
            archive.write_bytes(b'synthetic source')
            spec['sha256'] = sha(archive)
            (config / 'ffmpeg.json').write_text(json.dumps(spec))
            target = 'aarch64-apple-darwin'
            tools = {}
            for name in ('ffmpeg', 'ffprobe'):
                path = bindir / name
                path.write_bytes(b'synthetic ' + name.encode())
                tools[name] = dict(file=name, sha256=sha(path))
            manifest = dict(schema=1, target=target, ffmpeg_version=spec['version'],
                            license=spec['license'], configure=spec['configure'], tools=tools,
                            source={key: spec[key] for key in ('url', 'sha256')})
            (bindir / 'backend-manifest.json').write_text(json.dumps(manifest))
            with patch.object(package, 'ROOT', root):
                self.assertEqual(verify_backend(bindir, target, source_directory=resources), manifest)
                # A good adjacent archive cannot stand in for wrong package resources.
                (bindir / archive.name).write_bytes(archive.read_bytes())
                archive.write_bytes(b'wrong source')
                with self.assertRaisesRegex(ValueError, 'corresponding source absent or wrong'):
                    verify_backend(bindir, target, source_directory=resources)

    def test_source_download_retries_a_reset_and_publishes_only_verified_bytes(self):
        from build_ffmpeg import fetch, sha
        from hashlib import sha256
        from unittest.mock import patch
        from urllib.error import URLError

        data = b'verified synthetic source'
        with tempfile.TemporaryDirectory() as temp:
            archive = pathlib.Path(temp) / 'source.tar.xz'
            expected = sha256(data).hexdigest()
            with patch('build_ffmpeg.urllib.request.urlopen', side_effect=[
                URLError(ConnectionResetError()), io.BytesIO(data)
            ]) as request, patch('build_ffmpeg.time.sleep') as delay:
                fetch('https://example.invalid/source.tar.xz', archive, expected)
            self.assertEqual(request.call_count, 2)
            delay.assert_called_once_with(2)
            self.assertEqual(sha(archive), expected)
            self.assertEqual(list(archive.parent.iterdir()), [archive])

            archive.unlink()
            with patch('build_ffmpeg.urllib.request.urlopen', return_value=io.BytesIO(b'wrong')):
                with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                    fetch('https://example.invalid/source.tar.xz', archive, expected)
            self.assertEqual(list(archive.parent.iterdir()), [])

    def test_offline_source_requires_the_exact_prepared_archive(self):
        from build_ffmpeg import fetch
        from hashlib import sha256
        from unittest.mock import patch

        data = b'verified synthetic source'
        expected = sha256(data).hexdigest()
        with tempfile.TemporaryDirectory() as temp, patch('build_ffmpeg.urllib.request.urlopen') as request:
            archive = pathlib.Path(temp) / 'source.tar.xz'
            url = 'https://example.invalid/source.tar.xz'
            with self.assertRaises(FileNotFoundError):
                fetch(url, archive, expected, offline=True)
            archive.write_bytes(b'wrong')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                fetch(url, archive, expected, offline=True)
            archive.write_bytes(data)
            fetch(url, archive, expected, offline=True)
            request.assert_not_called()

class LicenseTests(unittest.TestCase):
    def test_author_names_are_not_a_grant_but_full_mit_authors_are(self):
        from licenses import applicable_grant
        self.assertFalse(applicable_grant('MIT', ['Authors: Synthetic Author']))
        grant=(pathlib.Path(__file__).resolve().parents[2]/'LICENSE').read_text()
        self.assertTrue(applicable_grant('MIT OR Apache-2.0', ['AUTHORS-MIT:\n'+grant]))
        self.assertFalse(applicable_grant('MPL-2.0', [grant]))
        self.assertFalse(applicable_grant('MIT AND Apache-2.0', [grant]))
        self.assertTrue(applicable_grant('(MIT OR Apache-2.0) AND MIT', [grant]))
    def test_registry_corresponding_source_must_match_lock_checksum(self):
        from licenses import source_archive
        import hashlib
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp);base=root/'registry/src/index/generic-1.0.0';base.mkdir(parents=True)
            archive=root/'registry/cache/index/generic-1.0.0.crate';archive.parent.mkdir(parents=True);archive.write_bytes(b'synthetic source archive')
            package={'name':'generic','version':'1.0.0'};expected=hashlib.sha256(archive.read_bytes()).hexdigest()
            receipt=source_archive(package,base,expected,root/'out')
            self.assertEqual((root/'out'/receipt['file']).read_bytes(),archive.read_bytes())
            with self.assertRaisesRegex(ValueError,'checksum mismatch'):source_archive(package,base,'0'*64,root/'bad')

class LinuxRuntimeTests(unittest.TestCase):
    def test_host_libraries_are_listed_without_redistribution(self):
        from package import linux_dependencies
        from unittest.mock import patch
        import subprocess
        with tempfile.TemporaryDirectory() as temp:
            stage=pathlib.Path(temp);(stage/'usr/bin').mkdir(parents=True);(stage/'usr/share/any-nle').mkdir(parents=True)
            (stage/'usr/bin/any-nle').write_bytes(b'synthetic executable')
            result=subprocess.CompletedProcess([],0,'libExample.so.1 => /usr/lib/libExample.so.1 (0x123)\n/lib64/ld-linux-x86-64.so.2 (0x456)\n')
            with patch('package.subprocess.run',return_value=result):linux_dependencies(stage)
            self.assertFalse((stage/'usr/lib').exists())
            receipt=json.loads((stage/'usr/share/any-nle/linux-runtime.json').read_text())
            self.assertEqual(receipt['binaries']['any-nle'],['ld-linux-x86-64.so.2','libExample.so.1'])
            with patch('package.subprocess.run',return_value=subprocess.CompletedProcess([],0,'libExample.so.1 => not found\n')):
                with self.assertRaises(ValueError):linux_dependencies(stage)


class WindowsImportTests(unittest.TestCase):
    def fixture(self,root,ordinary='KERNEL32.dll',delayed='api-ms-win-crt-runtime-l1-1-0.dll'):
        import struct
        data=bytearray(1024)
        def put(offset,fmt,value):struct.pack_into(fmt,data,offset,value)
        data[:2]=b'MZ';put(0x3c,'<I',0x80);data[0x80:0x84]=b'PE\0\0';put(0x86,'<H',1);put(0x94,'<H',240)
        optional=0x98;put(optional,'<H',0x20b);put(optional+24,'<Q',0x140000000);put(optional+60,'<I',512);put(optional+108,'<I',16)
        directories=optional+112;put(directories+8,'<I',0x1000);put(directories+12,'<I',40);put(directories+104,'<I',0x1040);put(directories+108,'<I',64)
        section=optional+240;put(section+8,'<I',512);put(section+12,'<I',0x1000);put(section+16,'<I',512);put(section+20,'<I',512)
        put(0x200+12,'<I',0x1080);put(0x240,'<I',1);put(0x244,'<I',0x10c0)
        data[0x280:0x280+len(ordinary)]=ordinary.encode();data[0x2c0:0x2c0+len(delayed)]=delayed.encode()
        path=root/'synthetic.exe';path.write_bytes(data);return path
    def test_ordinary_and_delay_imports_are_both_checked_without_toolchain(self):
        from windows_pe import verify
        with tempfile.TemporaryDirectory() as temp:
            root=pathlib.Path(temp);path=self.fixture(root)
            self.assertEqual(verify(path),['api-ms-win-crt-runtime-l1-1-0.dll','kernel32.dll'])
            path=self.fixture(root,delayed='VCRUNTIME140.dll')
            with self.assertRaisesRegex(ValueError,'non-system'):verify(path)
            path=self.fixture(root,ordinary='libgcc_s_seh-1.dll')
            with self.assertRaisesRegex(ValueError,'non-system'):verify(path)
            path=self.fixture(root,ordinary='../outside.dll')
            with self.assertRaisesRegex(ValueError,'invalid PE import'):verify(path)

if __name__=='__main__':unittest.main()
