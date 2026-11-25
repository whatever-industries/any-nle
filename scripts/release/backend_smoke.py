#!/usr/bin/env python3
"""Exercise the exact bundled tools with generated media."""
import argparse,json,pathlib,struct,subprocess,tempfile,zlib
from package import verify_backend

def check_png(root, ffprobe, run):
    image = root / 'synthetic-alpha.png'
    pixels = bytes((32, 64, 96, 0, 32, 64, 96, 128, 32, 64, 96, 255))

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

    image.write_bytes(b'\x89PNG\r\n\x1a\n'
        + chunk(b'IHDR', struct.pack('>IIBBBBB', 3, 1, 8, 6, 0, 0, 0))
        + chunk(b'IDAT', zlib.compress(b'\0' + pixels))
        + chunk(b'IEND', b''))
    probe = json.loads(subprocess.check_output([ffprobe, '-v', 'error', '-show_frames',
        '-show_entries', 'frame=width,height,pix_fmt', '-of', 'json', str(image)]))
    if probe.get('frames') != [{'width': 3, 'height': 1, 'pix_fmt': 'rgba'}]:
        raise ValueError('bundled ffprobe did not decode one RGBA PNG frame')
    decoded = run(['-i', str(image), '-map', '0:v:0', '-c:v', 'rawvideo', '-pix_fmt', 'rgba', '-f', 'rawvideo', '-'])
    if decoded != pixels:
        raise ValueError('bundled FFmpeg changed synthetic PNG pixels or alpha')
    return {'codec': 'png', 'container': 'png', 'decoded_rgba_equal': True}

def exercise(backend, target):
    """Run synthetic cases using the caller's verified backend directory."""
    backend=pathlib.Path(backend).resolve()
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
        cases.append(check_png(root, ffprobe, run))
    return {'schema':'anynle.backend-smoke/1','target':target,'synthetic_cases':cases,'passed':True}

def smoke(backend,target,output):
    verify_backend(backend,target)
    result=exercise(backend,target)
    pathlib.Path(output).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--backend',required=True);p.add_argument('--target',required=True);p.add_argument('--output',required=True);a=p.parse_args();smoke(a.backend,a.target,a.output)
