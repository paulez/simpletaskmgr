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

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn test_log_level_default_is_warn() {
        assert_eq!(log_level_for(&[s("simpletaskmgr")]), LevelFilter::Warn);
        assert_eq!(log_level_for(&[]), LevelFilter::Warn);
    }

    #[test]
    fn test_log_level_verbose_is_debug() {
        assert_eq!(
            log_level_for(&[s("simpletaskmgr"), s("-v")]),
            LevelFilter::Debug
        );
    }

    #[test]
    fn test_gtk_args_strips_verbose() {
        let out = gtk_args(&[s("simpletaskmgr"), s("-v")]);
        assert_eq!(out, vec![s("simpletaskmgr")]);
    }

    #[test]
    fn test_gtk_args_preserves_other_args_and_order() {
        let out = gtk_args(&[s("simpletaskmgr"), s("keepme"), s("-v"), s("also-keep")]);
        assert_eq!(out, vec![s("simpletaskmgr"), s("keepme"), s("also-keep")]);
    }

    #[test]
    fn test_gtk_args_without_verbose() {
        let out = gtk_args(&[s("simpletaskmgr"), s("--version")]);
        assert_eq!(out, vec![s("simpletaskmgr"), s("--version")]);
    }
}
