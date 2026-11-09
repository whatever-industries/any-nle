#!/usr/bin/env python3
"""Explicit synthetic-only public CI profile. It is not private-corpus acceptance."""
import argparse,json,os,pathlib,platform,re,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[1]
SUITES=[('gui',['-p','anynle-gui','--lib'],6),('inventory',['-p','anynle-app','--lib','job::inventory'],6 if os.name=='nt' else 7),('job_engine',['-p','anynle-app','--lib','job::engine'],9),('backend_bundle',['-p','anynle-app','--lib','backend_bundle'],2),('graph',['-p','anynle-graph','--lib'],27),('avid_adapter',['-p','anynle-avid-adapter','--lib'],27 if os.name=='nt' else 28),('mapping',['-p','anynle-mapping','--lib'],2)]
APP_TESTS=['rewrap::source::space_tests::available_space_queries_existing_volume','rewrap::source::space_tests::available_space_missing_path_refuses','rewrap::tests::existing_destination_is_refused_before_backend_or_inputs','rewrap::tests::pcm_counter_rejects_truncated_klv_and_counts_payload_bytes','rewrap::tests::private_klv_keys_are_allowed_only_inside_declared_header_metadata','rewrap::tests::exact_timecode_and_invalid_origins','rewrap::tests::mov_lane_admission_refuses_unqualified_width_before_source_access','rewrap::execution::ownership_tests::exclusive_creation_reports_only_the_owned_media_directory']
def run(output):
    output=pathlib.Path(output); output.mkdir(parents=True,exist_ok=True)
    results=[]
    selected=[(n,args,minimum,ROOT/'crates/Cargo.toml') for n,args,minimum in SUITES+[(n,['-p','anynle-app','--lib',n],1) for n in APP_TESTS]]
    selected += [(n,['--test',n],minimum,ROOT/'prproj/Cargo.toml') for n,minimum in [('original_media',3),('destination_layout',6)]]
    selected += [('source_open_cancellation',['--lib','source::progress_tests::cancellation_observer_runs_before_a_bin_is_opened'],1,ROOT/'avid-reader/Cargo.toml')]
    for name,args,minimum,manifest in selected:
        cmd=['cargo','test','--locked','--manifest-path',str(manifest),*args,'--','--nocapture']
        if name in APP_TESTS or name == 'source_open_cancellation': cmd.append('--exact')
        proc=subprocess.run(cmd,cwd=ROOT,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        log=proc.stdout; (output/(name.replace(':','_')+'.log')).write_text(log)
        counts=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',log)
        passed=sum(int(a) for a,b,c in counts)
        if proc.returncode or passed<minimum or any(int(b) or int(c) for a,b,c in counts) or re.search(r'(?im)^.*\b(skip|skipped|skipping)\b',log):
            print(log); raise ValueError('public profile failed or incomplete: '+name)
        results.append({'suite':name,'passed':passed,'minimum':minimum,'failed':0,'ignored':0})
    receipt={'schema':'anynle.public-acceptance/1','profile':'synthetic-only','platform':platform.system(),'architecture':platform.machine(),'platform_exclusions':['Unix-only symlink and /bin/sh child-process controls'] if os.name=='nt' else [],'private_corpus_qualification':False,'suites':results,'passed':sum(r['passed'] for r in results)}
    (output/'public-acceptance.json').write_text(json.dumps(receipt,indent=2)+'\n'); print(json.dumps(receipt))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',required=True);run(p.parse_args().output)
