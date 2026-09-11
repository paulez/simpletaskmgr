//! Binary entry point for Simple Task Manager.
use log::{info, LevelFilter};
use simplelog::*;

use gtk4::prelude::*;

/// The `--no-resort-kick` diagnostic flag: when present, the refresh path
/// leaves the CPU-sorted display order untouched after in-place value updates
/// (rows keep their positions until a membership change re-sorts), so a
/// flicker test can attribute visible refresh flicker to the re-sort commit
/// versus the membership-update path. Stripped from the arguments GTK sees.
const NO_RESORT_KICK: &str = "--no-resort-kick";

/// Resolve the log level from the program arguments.
///
/// `-v` (verbose) enables `debug`; otherwise the default is `warn`, so
/// running the app as-is stays quiet on stdout.
fn log_level_for(args: &[String]) -> LevelFilter {
    if args.iter().any(|a| a == "-v") {
        LevelFilter::Debug
    } else {
        LevelFilter::Warn
    }
}

/// Report whether the user passed the `--no-resort-kick` diagnostic flag.
fn no_resort_kick(args: &[String]) -> bool {
    args.iter().any(|a| a == NO_RESORT_KICK)
}

/// Return the arguments GTK should see, with our own flags (`-v` and
/// `--no-resort-kick`) removed so `GApplication` never tries to parse them.
/// `argv[0]` is preserved.
fn gtk_args(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|a| *a != "-v" && *a != NO_RESORT_KICK)
        .cloned()
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let log_config = ConfigBuilder::new()
        .add_filter_allow_str("simpletaskmgr")
        .build();
    let _ = SimpleLogger::init(log_level_for(&args), log_config);
    info!("Starting simpletaskmgr");

    let gtk_argv = gtk_args(&args);
    let no_kick = no_resort_kick(&args);

    let app = gtk4::Application::builder()
        .application_id("org.simpletaskmgr.simpletaskmgr")
        .build();
    app.connect_activate(move |app| {
        let win = simpletaskmgr::ui::build_window(app, no_kick);
        win.set_visible(true);
    });
    let _ = app.run_with_args(&gtk_argv);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// `log_level_for` maps to Debug with `-v`, Warn otherwise (the default).
    #[rstest]
    #[case::default("simpletaskmgr", LevelFilter::Warn)]
    #[case::empty("", LevelFilter::Warn)]
    #[case::verbose("simpletaskmgr -v", LevelFilter::Debug)]
    fn test_log_level_for(#[case] args_str: &str, #[case] expected: LevelFilter) {
        let args: Vec<String> = args_str.split_whitespace().map(String::from).collect();
        assert_eq!(log_level_for(&args), expected);
    }

    /// `gtk_args` strips `-v` and `--no-resort-kick` (kept for our own use)
    /// and passes every other argument through in order.
    #[rstest]
    #[case::strips_verbose("simpletaskmgr -v", "simpletaskmgr")]
    #[case::preserves_order("simpletaskmgr keepme -v also-keep", "simpletaskmgr keepme also-keep")]
    #[case::no_verbose("simpletaskmgr --version", "simpletaskmgr --version")]
    #[case::strips_no_resort_kick("simpletaskmgr --no-resort-kick -v", "simpletaskmgr")]
    fn test_gtk_args(#[case] input: &str, #[case] expected: &str) {
        let args: Vec<String> = input.split_whitespace().map(String::from).collect();
        let out = gtk_args(&args);
        let expected_vec: Vec<String> = expected.split_whitespace().map(String::from).collect();
        assert_eq!(out, expected_vec);
    }

    /// `no_resort_kick` detects the diagnostic flag wherever it appears.
    #[rstest]
    #[case::absent("simpletaskmgr", false)]
    #[case::present("simpletaskmgr --no-resort-kick", true)]
    #[case::present_after_other("simpletaskmgr -v --no-resort-kick", true)]
    fn test_no_resort_kick(#[case] input: &str, #[case] expected: bool) {
        let args: Vec<String> = input.split_whitespace().map(String::from).collect();
        assert_eq!(no_resort_kick(&args), expected);
    }
}
