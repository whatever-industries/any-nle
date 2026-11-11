import gzip,io,json,pathlib,sys,tempfile,unittest,zipfile,zlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
from audit import Scanner
from package import verify_backend
from publish import validate_existing
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
class PublicationTests(unittest.TestCase):
    def test_published_version_and_wrong_source_cannot_be_reused(self):
        validate_existing([], 'a'*40)
        validate_existing([{'draft':True,'target_commitish':'a'*40}], 'a'*40)
        for r in ({'draft':False,'target_commitish':'a'*40},{'draft':True,'target_commitish':'b'*40}):
            with self.assertRaises(ValueError):validate_existing([r], 'a'*40)

class BackendTests(unittest.TestCase):
    def test_missing_backend_refuses(self):
        with tempfile.TemporaryDirectory() as t:
            with self.assertRaises(FileNotFoundError):verify_backend(pathlib.Path(t),'aarch64-apple-darwin')

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
