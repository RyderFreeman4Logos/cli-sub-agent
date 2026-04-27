use csa_core::vcs::VcsBackend;
use csa_session::JjJournal;
use std::path::Path;

fn needs_vcs_backend(_backend: &dyn VcsBackend) {}

fn main() {
    let journal = JjJournal::new(Path::new(".")).expect("journal should construct");
    needs_vcs_backend(&journal);
}
