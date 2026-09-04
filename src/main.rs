//! Binary entry point for Simple Task Manager.
use log::{info, LevelFilter};
use simplelog::*;

use gtk4::prelude::*;

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

/// Return the arguments GTK should see, with our own `-v` flag removed so
/// `GApplication` never tries to parse it. `argv[0]` is preserved.
fn gtk_args(args: &[String]) -> Vec<String> {
    args.iter().filter(|a| *a != "-v").cloned().collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let log_config = ConfigBuilder::new()
        .add_filter_allow_str("simpletaskmgr")
        .build();
    let _ = SimpleLogger::init(log_level_for(&args), log_config);
    info!("Starting simpletaskmgr");

    let app = gtk4::Application::builder()
        .application_id("org.simpletaskmgr.simpletaskmgr")
        .build();
    app.connect_activate(|app| {
        let win = simpletaskmgr::ui::build_window(app);
        win.set_visible(true);
    });
    let _ = app.run_with_args(&gtk_args(&args));
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

    /// `gtk_args` strips any `-v` (kept for our own logger) and passes every
    /// other argument through in order.
    #[rstest]
    #[case::strips_verbose("simpletaskmgr -v", "simpletaskmgr")]
    #[case::preserves_order("simpletaskmgr keepme -v also-keep", "simpletaskmgr keepme also-keep")]
    #[case::no_verbose("simpletaskmgr --version", "simpletaskmgr --version")]
    fn test_gtk_args(#[case] input: &str, #[case] expected: &str) {
        let args: Vec<String> = input.split_whitespace().map(String::from).collect();
        let out = gtk_args(&args);
        let expected_vec: Vec<String> = expected.split_whitespace().map(String::from).collect();
        assert_eq!(out, expected_vec);
    }
}
