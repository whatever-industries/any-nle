//! Effective Premiere construction, resolved after policy overrides.
//! Internal to the CLI: model options and public writer interfaces stay unchanged.
use mapping::BinMode;
use prproj::convert::model::Model;
use prproj::convert::rewrap::MediaRelink;
use prproj::convert::writer::WriteError;
use prproj::UuidSource;
use std::fmt;
use std::path::Path;

#[derive(Clone, Copy)]
pub enum PremiereRoute {
    Bin(BinMode),
    NativeOp1a,
    NativeMedia,
}

impl PremiereRoute {
    pub fn prepare(
        self,
        model: &Model,
        output: &Path,
        ids: &mut dyn UuidSource,
        bindings: &[MediaRelink],
        editorial: prproj::convert::writer::Editorial<'_>,
    ) -> Result<prproj::convert::writer::PreparedProject, WriteError> {
        prproj::convert::writer::prepare_project_with_effects(
            model,
            editorial,
            output,
            ids,
            bindings,
            matches!(self, Self::NativeOp1a | Self::NativeMedia),
        )
    }
    pub fn resolve(native_master: bool, bin_mode: BinMode) -> Self {
        if native_master {
            Self::NativeOp1a
        } else {
            Self::Bin(bin_mode)
        }
    }

    pub fn preflight(self, model: &Model) -> Result<(), WriteError> {
        match self {
            Self::NativeOp1a | Self::NativeMedia => prproj::convert::native_av::preflight(model),
            Self::Bin(_) => Ok(()),
        }
    }

    pub fn notice(self) -> Option<&'static str> {
        match self {
            Self::NativeOp1a => Some("Experimental native OP1a-backed AV masters: replaces bin mode; Premiere acceptance pending."),
            Self::NativeMedia => Some("Experimental native prepared-media AV masters: DNx OP1a / ProRes MOV; replaces bin mode; User acceptance pending."),
            Self::Bin(_) => None,
        }
    }

    pub fn receipt_mode(self) -> &'static str {
        match self {
            Self::NativeOp1a => "experimental native OP1a AV master",
            Self::NativeMedia => {
                "experimental native prepared-media AV master (DNx OP1a / ProRes MOV)"
            }
            Self::Bin(mode) => mode.as_str(),
        }
    }
}

impl fmt::Display for PremiereRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Bin(_) = self {
            f.write_str("bin mode ")?;
        }
        f.write_str(self.receipt_mode())
    }
}
