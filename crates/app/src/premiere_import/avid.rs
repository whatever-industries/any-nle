//! Reconcile Premiere's media record with the verified Avid-linked draft.
use prproj::convert::writer::PreparedProject;

use super::{worker, Result};

pub(crate) fn bind(document: &mut PreparedProject) -> Result<()> {
    // The worker owns one bounded request at a time. Publish nothing until
    // every captured record is reconciled with the same staged project.
    let paths: Vec<_> = document.assisted_originals().cloned().collect();
    let donor = worker::capture(&paths)?;
    document
        .verify_assisted_media(Some(&donor.path))
        .map_err(|error| error.to_string())
}
