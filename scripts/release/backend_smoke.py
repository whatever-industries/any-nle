#!/usr/bin/env python3
"""Exercise the exact bundled tools with two entirely generated media cases."""
import argparse,hashlib,json,pathlib,subprocess,tempfile
from package import verify_backend

def smoke(backend,target,output):
    backend=pathlib.Path(backend).resolve();verify_backend(backend,target)
    ext='.exe' if 'windows' in target else ''
    ffmpeg=str(backend/('ffmpeg'+ext));ffprobe=str(backend/('ffprobe'+ext));cases=[]
    def run(args):return subprocess.check_output([ffmpeg,'-v','error','-nostdin',*args])
    with tempfile.TemporaryDirectory(prefix='any-nle-synthetic-backend-') as temp:
        root=pathlib.Path(temp)
        for codec,container in [('dnxhd','mxf'),('prores_ks','mov')]:
            original=root/('synthetic-'+codec+'.'+container);copy=root/('copied-'+codec+'.'+container)
            args=['-f','lavfi','-i','testsrc2=size=1920x1080:rate=24','-f','lavfi','-i','sine=frequency=440:sample_rate=48000','-map','0:v','-map','1:a','-frames:v','3','-t','0.125','-c:v',codec]
            args+=['-profile:v','dnxhr_lb'] if codec=='dnxhd' else ['-profile:v','1','-pix_fmt','yuv422p10le']
            args+=['-c:a','pcm_s24le','-timecode','01:00:00:00',str(original)]
            run(args);run(['-i',str(original),'-map','0','-c','copy',str(copy)])
            info=json.loads(subprocess.check_output([ffprobe,'-v','error','-show_streams','-of','json',str(copy)]))
            if [s['codec_type'] for s in info['streams']].count('video')!=1 or [s['codec_type'] for s in info['streams']].count('audio')!=1:raise ValueError('synthetic stream count mismatch')
            for stream in ('v:0','a:0'):
                def essence(path):
                    return run(['-i',str(path),'-map','0:'+stream,'-f','rawvideo' if stream[0]=='v' else 's24le','-c','rawvideo' if stream[0]=='v' else 'pcm_s24le','-'])
                before,after=essence(original),essence(copy)
                if not before or before!=after:raise ValueError('synthetic rewrap decoded essence changed')
            cases.append({'codec':codec,'container':container,'decoded_picture_and_pcm_equal':True})
    result={'schema':'anynle.backend-smoke/1','target':target,'synthetic_cases':cases,'passed':True}
    pathlib.Path(output).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--backend',required=True);p.add_argument('--target',required=True);p.add_argument('--output',required=True);a=p.parse_args();smoke(a.backend,a.target,a.output)
