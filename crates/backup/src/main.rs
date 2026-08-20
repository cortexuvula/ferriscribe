//! `ferriscribe-backup` CLI entry point.
//!
//! Subcommands are wired in progressively; this stub prints usage so the
//! binary target builds from the first commit of the crate.

fn main() {
    eprintln!("ferriscribe-backup — encrypted off-machine backup for FerriScribe");
    eprintln!(
        "usage: ferriscribe-backup <backup|restore|verify|drill|escrow|push|pull|serve|install-schedule> [options]"
    );
    eprintln!("(this build has no subcommands wired yet)");
    std::process::exit(2);
}
