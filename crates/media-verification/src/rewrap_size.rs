//! An output recipe counts its source file lengths once per output.
use std::{collections::HashMap, fs, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub bytes: u64,
    pub outputs: usize,
}

/// Metadata only. Callers supply one recipe per distinct output, in stream order.
pub fn calculate(recipes: &[Vec<PathBuf>]) -> Result<Estimate, String> {
    let mut lengths: HashMap<&PathBuf, Result<u64, String>> = HashMap::new();
    let mut bytes = 0u64;
    let mut missing = Vec::new();
    for path in recipes.iter().flatten() {
        match lengths.entry(path).or_insert_with(|| {
            fs::metadata(path)
                .and_then(|metadata| {
                    if metadata.is_file() {
                        Ok(metadata.len())
                    } else {
                        Err(std::io::Error::other("expected a regular file"))
                    }
                })
                .map_err(|error| format!("{}: {error}", path.display()))
        }) {
            Ok(len) => {
                bytes = bytes
                    .checked_add(*len)
                    .ok_or("Rewrap size exceeds the supported range.")?
            }
            Err(error) if !missing.contains(error) => missing.push(error.clone()),
            Err(_) => {}
        }
    }
    if !missing.is_empty() {
        return Err(missing.join("\n"));
    }
    Ok(Estimate {
        bytes,
        outputs: recipes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_shared_by_two_outputs_counts_in_each() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let lengths = fs::metadata(&source).unwrap().len();
        assert_eq!(
            calculate(&[vec![source.clone()], vec![source]]),
            Ok(Estimate {
                bytes: 2 * lengths,
                outputs: 2
            })
        );
    }

    #[test]
    fn unavailable_inputs_are_named() {
        let absent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("absent-rewrap-input.mxf");
        let error = calculate(&[vec![absent.clone()], vec![absent]]).unwrap_err();
        assert_eq!(error.matches("absent-rewrap-input.mxf").count(), 1);
    }
}
