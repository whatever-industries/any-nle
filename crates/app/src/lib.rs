//! Shared Any NLE application engine. The CLI and native wizard share preparation and publication.
mod application;
mod media_prompt;
mod output;
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
