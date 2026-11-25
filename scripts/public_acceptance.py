#!/usr/bin/env python3
"""Explicit synthetic-only public CI profile. It is not private-corpus acceptance."""
import argparse,json,os,pathlib,platform,re,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[1]
SUITES=[('gui',['-p','anynle-gui','--lib'],6),('inventory',['-p','anynle-app','--lib','job::inventory'],6 if os.name=='nt' else 7),('job_engine',['-p','anynle-app','--lib','job::engine'],9),('backend_bundle',['-p','anynle-app','--lib','backend_bundle'],2),('graph',['-p','anynle-graph','--lib'],27),('avid_adapter',['-p','anynle-avid-adapter','--lib'],27 if os.name=='nt' else 28),('mapping',['-p','anynle-mapping','--lib'],2)]
APP_TESTS=['rewrap::source::space_tests::available_space_queries_existing_volume','rewrap::source::space_tests::available_space_missing_path_refuses','rewrap::tests::existing_destination_is_refused_before_backend_or_inputs','rewrap::tests::pcm_counter_rejects_truncated_klv_and_counts_payload_bytes','rewrap::tests::private_klv_keys_are_allowed_only_inside_declared_header_metadata','rewrap::tests::exact_timecode_and_invalid_origins','rewrap::tests::mov_lane_admission_refuses_unqualified_width_before_source_access','rewrap::execution::ownership_tests::exclusive_creation_reports_only_the_owned_media_directory']
APP_TESTS += ['rewrap::mov::tests::prores_alpha_rewrite_changes_only_qualified_sample_depth']
APP_TESTS += ['rewrap::mov::tests::anamorphic_picture_requires_matching_saved_and_physical_geometry']
APP_TESTS += ['rewrap::mono::tests::wide_wav_stores_only_exact_source_sample_references']
APP_TESTS += ['rewrap::mono::tests::size_review_lists_each_published_media_recipe']
APP_TESTS += ['rewrap::tests::rewrap_preview_splits_distinct_wide_bundles_and_includes_mono_gain']
APP_TESTS += ['rewrap::tests::mono_rewrap_reports_exact_file_failure_separately_from_recipe_error']
APP_TESTS += ['avid_media::master::tests::managed_master_places_referenced_objects_before_the_bin_root']
APP_TESTS += ['avid_media::linked::markers::tests::point_marker_requires_dependencies_before_its_owner']
APP_TESTS += ['rewrap::terminal::tests::managed_dnxhd_requires_indexed_cid_1253_and_zero_alignment_tail']
APP_TESTS += ['avid_media::linked::transition::tests::writes_centered_transition_with_owned_source_handles']
APP_TESTS += ['avid_media::linked::native_bin::tests::timeline_uses_the_prepared_source_and_track_for_each_edit']
APP_TESTS += ['png_mattes::tests::png_gap_recovery_requires_exact_occurrence_master_and_range']
RESOLVE_SOURCE_TESTS = [
    'premiere::capture::physical::tests::timecoded_avc_aac_candidate_requires_exact_encoding_and_stream_roles',
    'archive::tests::bundled_resolve_templates_bind_the_reviewed_role_bytes',
    'source::multicam::tests::multipart_picture_angle_keeps_both_trims_and_the_gap',
    'source::multicam::tests::single_group_audio_slot_preserves_each_virtual_channel',
    'source::tests::imported_movie_split_selectors_follow_physical_channel_order',
    'source::tests::embedded_movie_overrides_physical_audio_channel_without_changing_track',
    'source::tests::embedded_aac_keeps_44100_sample_clock_and_saved_channel_route',
    'source::tests::temporary_archive_keeps_project_validation',
    'source::tests::standalone_pcm_wav_captures_physical_channels_without_opening_original',
    'source::encoding::tests::embedded_reel_mode_preserves_other_setup_fields',
    'source::timing::tests::clock_precision_checks_the_known_tick',
    'source::timing::tests::saved_frame_clock_divides_by_the_binary64_rate',
    'source::timing::tests::frame_fraction_retains_signed_binary64_and_rejects_bad_payloads',
    'source::timing::tests::fractional_group_duration_has_an_exact_source_extent',
]
def run(output):
    output=pathlib.Path(output); output.mkdir(parents=True,exist_ok=True)
    results=[]; failures=[]
    workspace=ROOT/'crates/Cargo.toml'
    selected=[(n,args,minimum,workspace) for n,args,minimum in SUITES+[(n,['-p','anynle-app','--lib',n],1) for n in APP_TESTS]]
    selected += [
        ('gui_bin_picker', ['-p', 'anynle-gui', '--bin', 'any-nle-gui', 'wizard::bin_picker::tests'], 4, workspace),
        ('gui_report', ['-p', 'anynle-gui', '--bin', 'any-nle-gui', 'report'], 3, workspace),
        ('gui_svg_arrow', ['-p', 'anynle-gui', '--bin', 'any-nle-gui', 'icons::tests'], 1, workspace),
        ('gui_diagnostics', ['-p', 'anynle-gui', '--bin', 'any-nle-gui', 'diagnostics_review_and_activity'], 1, workspace),
        ('diagnostics', ['-p', 'anynle-app', '--lib', 'diagnostics::tests'], 1, workspace),
        ('support_mcp', ['-p', 'anynle-app', '--test', 'support_mcp'], 2, workspace),
        ('media_verification', ['-p', 'anynle-media-verification', '--lib'], 4, workspace),
        ('source_input', ['-p', 'anynle-app', '--lib', 'source_input::tests'], 3 if os.name == 'nt' else 4, workspace),
        ('resolve_source', ['-p', 'anynle-resolve-adapter', '--lib'], len(RESOLVE_SOURCE_TESTS), workspace),
        ('premiere_preparation', ['-p', 'anynle-premiere-adapter', '--test', 'preparation'], 54, workspace),
        ('premiere_import_worker', ['-p', 'anynle-app', '--lib', 'premiere_import::worker'], 2, workspace),
        ('graph_output', ['-p', 'anynle-app', '--test', 'graph_output'], 11 if os.name == 'nt' else 13, workspace),
        ('avid_processing', ['-p', 'anynle-app', '--lib', 'avid_processing'], 1, workspace),
    ]
    if platform.system() == 'Darwin':
        selected.append(('premiere_review', ['-p', 'anynle-gui', '--bin', 'any-nle-gui', 'premiere_review'], 1, workspace))
    selected += [(n,['--test',n],minimum,ROOT/'prproj/Cargo.toml') for n,minimum in [('original_media',3),('destination_layout',6)]]
    selected += [('source_open_cancellation',['--lib','source::progress_tests::cancellation_observer_runs_before_a_bin_is_opened'],1,ROOT/'avid-reader/Cargo.toml')]
    for name,args,minimum,manifest in selected:
        cmd=['cargo','test','--locked','--manifest-path',str(manifest),*args,'--','--nocapture']
        if name in APP_TESTS or name == 'source_open_cancellation': cmd.append('--exact')
        if name == 'resolve_source': cmd.extend(['--exact', *RESOLVE_SOURCE_TESTS])
        proc=subprocess.run(cmd,cwd=ROOT,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        log=proc.stdout; (output/(name.replace(':','_')+'.log')).write_text(log)
        counts=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',log)
        passed=sum(int(a) for a,b,c in counts)
        if proc.returncode or passed<minimum or any(int(b) or int(c) for a,b,c in counts) or re.search(r'(?im)^.*\b(skip|skipped|skipping)\b',log):
            print(log)
            failures.append(name)
            continue
        results.append({'suite':name,'passed':passed,'minimum':minimum,'failed':0,'ignored':0})
    if failures:
        raise ValueError('public profile failed or incomplete: '+', '.join(failures))
    receipt={'schema':'anynle.public-acceptance/1','profile':'synthetic-only','platform':platform.system(),'architecture':platform.machine(),'platform_exclusions':['Unix-only symlink and /bin/sh child-process controls'] if os.name=='nt' else [],'private_corpus_qualification':False,'suites':results,'passed':sum(r['passed'] for r in results)}
    (output/'public-acceptance.json').write_text(json.dumps(receipt,indent=2)+'\n'); print(json.dumps(receipt))
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',required=True);run(p.parse_args().output)
