#!/usr/bin/env python3
"""Fail closed on credentials or caller-supplied private tokens, including archives.

Private dictionaries are read from a local external file and never echoed. The
scanner emits only counts and hashed locations, suitable for private audit receipts.
"""
import argparse,gzip,hashlib,io,json,pathlib,re,tarfile,zipfile,zlib
CREDENTIALS=[re.compile(rb'-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----\r?\n(?:[A-Za-z0-9+/=]+\r?\n)+-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----'),re.compile(rb'gh[pousr]_[A-Za-z0-9]{30,}'),re.compile(rb'github_pat_[A-Za-z0-9_]{30,}'),re.compile(rb'AKIA[0-9A-Z]{16}')]
class Scanner:
    def __init__(self,tokens=(),budget=2*1024**3,known_upstream=()):
        self.tokens=[]
        for token in tokens:
            if len(token.strip())<3: raise ValueError('private token too short')
            for encoding in ('utf-8','utf-16-le','utf-16-be'):
                self.tokens.append(token.encode(encoding).lower())
        self.budget=budget;self.bytes=0;self.objects=0;self.hits=[];self.undecodable=[];self.known_upstream={f["sha256"]:f for f in known_upstream}
    def scan(self,data,location='root',depth=0):
        if depth>10: raise ValueError('archive recursion budget exceeded')
        self.bytes+=len(data);self.objects+=1
        if self.bytes>self.budget: raise ValueError('expanded audit byte budget exceeded')
        lower=data.lower()
        count=sum(t in lower for t in self.tokens)+sum(bool(p.search(data)) for p in CREDENTIALS)
        if count:self.hits.append({'location_sha256':hashlib.sha256(location.encode()).hexdigest(),'matches':count})
        stream=io.BytesIO(data)
        if zipfile.is_zipfile(stream):
            stream.seek(0)
            with zipfile.ZipFile(stream) as z:
                for item in z.infolist():
                    if item.is_dir():continue
                    if item.file_size>self.budget-self.bytes:raise ValueError('ZIP expanded byte budget exceeded')
                    self.scan(z.read(item),location+'!'+item.filename,depth+1)
            return
        stream.seek(0)
        try:t=tarfile.open(fileobj=stream,mode='r:*')
        except tarfile.ReadError:t=None
        if t:
            with t:
                for item in t:
                    if item.isfile():
                        if item.size>self.budget-self.bytes:raise ValueError('TAR expanded byte budget exceeded')
                        self.scan(t.extractfile(item).read(),location+'!'+item.name,depth+1)
            return
        if data.startswith(b'\x1f\x8b'):
            try:
                with gzip.GzipFile(fileobj=io.BytesIO(data)) as g: raw=g.read(self.budget-self.bytes+1)
                self.scan(raw,location+'!gzip',depth+1)
            except (gzip.BadGzipFile,EOFError,zlib.error) as error:
                digest=hashlib.sha256(data).hexdigest()
                known=self.known_upstream.get(digest)
                accepted=bool(known and known.get('bytes')==len(data))
                self.undecodable.append({'location_sha256':hashlib.sha256(location.encode()).hexdigest(),'content_sha256':digest,'format':'gzip','error_type':type(error).__name__,'accepted_upstream_fixture':accepted})
        # Project/model fixtures may be zlib streams embedded after a binary header.
        offsets=[m.start() for m in re.finditer(rb'\x78[\x01\x5e\x9c\xda]',data)]
        if len(offsets)>10000:raise ValueError('zlib candidate budget exceeded')
        for offset in offsets:
            try:
                d=zlib.decompressobj();raw=d.decompress(data[offset:],self.budget-self.bytes+1)
                if d.eof:self.scan(raw,location+'!zlib@'+str(offset),depth+1)
                elif d.unconsumed_tail:raise ValueError('zlib expanded byte budget exceeded')
            except zlib.error:pass
    def report(self):return {'schema':'anynle.privacy-audit/1','objects':self.objects,'expanded_bytes':self.bytes,'match_locations':self.hits,'undecodable_archives':self.undecodable,'expansion_complete':not self.undecodable,'passed':not self.hits and all(x['accepted_upstream_fixture'] for x in self.undecodable)}
def audit(paths,tokens=(),known_upstream=()):
    scanner=Scanner(tokens,known_upstream=known_upstream)
    for path in paths:
        path=pathlib.Path(path)
        for p in sorted(path.rglob('*')) if path.is_dir() else [path]:
            if p.is_file():scanner.scan(p.read_bytes(),str(p))
    return scanner.report()
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('paths',nargs='+');p.add_argument('--private-token-file');p.add_argument('--known-upstream-fixtures');p.add_argument('--output',required=True);a=p.parse_args()
    tokens=pathlib.Path(a.private_token_file).read_text().splitlines() if a.private_token_file else []
    known=[]
    if a.known_upstream_fixtures:
        manifest=json.loads(pathlib.Path(a.known_upstream_fixtures).read_text())
        if manifest.get('schema')!='anynle.known-upstream-archive-fixtures/1':raise ValueError('unknown upstream fixture manifest')
        known=manifest['fixtures']
    result=audit(a.paths,tokens,known);pathlib.Path(a.output).write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k!='match_locations'}));raise SystemExit(0 if result['passed'] else 1)
