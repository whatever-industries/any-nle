//! Shared Any NLE application engine. The CLI and native wizard share preparation and publication.
mod application;
mod png_mattes;
mod wav_original;
mod media_prompt;
mod offline;
mod output;
mod premiere_preparation;
mod premiere_route;
mod rewrap;
mod source_input;
pub mod job;
pub mod backend_bundle;

pub fn cli_main()->std::process::ExitCode { application::main() }

#[cfg(test)]
use application::{Args,parse,convert,USAGE};
#[cfg(test)]
use premiere_route::PremiereRoute;
#[cfg(test)]
mod rewrap_args_tests {
    use super::*;
    fn args(xs: &[&str]) -> Result<Args, String> {
        parse(&xs.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn dry_run_cannot_be_swallowed_as_an_output_path() {
        assert!(args(&["--rewrap-op1a", "--rewrap-dry-run"]).is_err());
        assert!(args(&["--rewrap-dry-run"]).is_err());
        assert!(args(&["--ffmpeg", "ffmpeg"]).is_err());
    }
    #[test]
    fn mixed_codec_media_requires_native_and_exclusive_destination_choice() {
        assert!(args(&["--rewrap-media","new"]).is_err());
        let a=args(&["--rewrap-media","new","--rewrap-native-master","--rewrap-dry-run"]).unwrap();
        assert!(a.rewrap_media && a.wants_rewrap() && a.rewrap_dry_run);
        assert!(args(&["--rewrap-op1a","old","--rewrap-media","new","--rewrap-native-master"]).is_err());
        assert!(args(&["--rewrap-media","--rewrap-native-master"]).is_err());
        assert!(PremiereRoute::NativeMedia.receipt_mode().contains("ProRes MOV"));
    }
    #[test]
    fn duplicate_rewrap_directory_is_refused() {
        assert!(args(&["--rewrap-op1a", "one", "--rewrap-op1a", "two"]).is_err());
    }
    #[test]
    fn original_link_mode_requires_premiere_and_explicit_root_mappings() {
        let root = std::env::current_dir().unwrap();
        let root = root.to_str().unwrap();
        let a = args(&[
            "--to", "premiere", "out.prproj", "--link-originals",
            "--map-original-root", "/saved", root, "--ffprobe", "probe",
        ]).unwrap();
        assert!(a.link_originals && !a.wants_rewrap());
        assert_eq!(a.original_mappings[0].current_root, std::path::Path::new(root));
        assert!(args(&["--map-original-root", "/saved", root]).is_err());
        for options in [
            vec!["--map-original-root", "/saved", "--ffprobe", "probe"],
            vec!["--map-original-root", "/saved", "relative"],
            vec!["--rewrap-op1a", "new"],
            vec!["--link-originals"],
        ] {
            let mut command = vec!["--to", "premiere", "out.prproj", "--link-originals"];
            command.extend(options);
            assert!(args(&command).is_err());
        }
        assert!(args(&["--to", "fcpxml", "out.fcpxml", "--link-originals"]).is_err());
    }
    #[test]
    fn offline_media_is_opt_in_and_refuses_rewrap() {
        let direct = ["--to", "premiere", "out.prproj"];
        assert!(!args(&direct).unwrap().allow_offline_media);
        for mode in ["separate", "grouped", "merged"] {
            assert!(args(&[&direct[..], &["--allow-offline-media", "--bin-mode", mode]].concat())
                .unwrap().allow_offline_media);
        }
        assert!(args(&[&direct[..], &["--allow-offline-media", "--rewrap-op1a-prompt"]].concat()).is_err());
        let old: crate::job::JobSpec = serde_json::from_str(r#"{"mode":"separate"}"#).unwrap();
        assert!(!old.allow_offline_media);
        assert!(!serde_json::to_string(&old).unwrap().contains("allow_offline_media"));
    }
    #[test]
    fn prompt_is_an_explicit_exclusive_rewrap_choice() {
        let a = args(&[
            "--rewrap-op1a-prompt",
            "--rewrap-dry-run",
            "--ffmpeg",
            "custom",
        ])
        .unwrap();
        assert!(a.wants_rewrap() && a.rewrap_prompt && a.rewrap_dry_run);
        for options in [
            vec!["--rewrap-op1a-prompt", "--rewrap-op1a-prompt"],
            vec!["--rewrap-op1a-prompt", "--rewrap-op1a", "new"],
            vec!["--rewrap-op1a", "new", "--rewrap-op1a-prompt"],
        ] {
            assert!(args(&options).is_err());
        }
        let mut a = a;
        a.to = Some("fcpxml".into());
        assert!(convert(&a).unwrap_err().contains("only supported"));
    }
    #[test]
    fn native_master_requires_rewrap_and_an_unambiguous_choice() {
        assert!(args(&["--rewrap-native-master"]).is_err());
        assert!(args(&[
            "--rewrap-op1a",
            "new",
            "--rewrap-native-master",
            "--rewrap-native-master"
        ])
        .is_err());
        assert!(args(&[
            "--rewrap-op1a",
            "new",
            "--rewrap-native-master",
            "--bin-mode",
            "grouped"
        ])
        .is_err());
        let a = args(&[
            "--rewrap-op1a-prompt",
            "--rewrap-native-master",
            "--rewrap-dry-run",
        ])
        .unwrap();
        assert!(a.rewrap_native_master && a.rewrap_dry_run && a.rewrap_prompt);
    }
    #[test]
    fn notice_and_non_premiere_guard() {
        assert!(USAGE.contains("Creates new media."));
        let mut a = Args::default();
        a.to = Some("fcpxml".into());
        a.rewrap = Some("never-created".into());
        assert!(convert(&a).unwrap_err().contains("only supported"));
    }
}
