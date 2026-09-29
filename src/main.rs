//! Binary entry point for Simple Task Manager.
use log::{info, LevelFilter};
use simplelog::*;

use gtk4::prelude::*;

/// Application id, used as the `GApplication` id **and** the file name of the
/// desktop entry in `share/`. Shells attribute the running window to the
/// entry (Wayland: by exactly this name; X11: via WM_CLASS, which GTK
/// derives from it) and show its `Icon=`, so the entry file must stay named
/// `<APP_ID>.desktop`. The tests below pin both properties, and the
/// `Icon=` ↔ window-icon-name link is pinned in `src/ui.rs`.
pub const APP_ID: &str = "org.simpletaskmgr.simpletaskmgr";

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

/// Return the arguments GTK should see, with our own flag (`-v`) removed so `GApplication` never tries to parse it. `argv[0]` is preserved.
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

    let gtk_argv = gtk_args(&args);

    let app = gtk4::Application::builder().application_id(APP_ID).build();
    // Every `activate` call (initial launch *and* a second launch, which is
    // forwarded to this single-instance app) shares this one window registry:
    // a duplicate launch presents the existing window instead of building a
    // second state.
    let window_ref = simpletaskmgr::ui::WindowRef::new();
    let wr = window_ref.clone();
    app.connect_activate(move |a| {
        wr.activate(a);
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

    /// `gtk_args` strips `-v` (kept for our own use) and passes every other
    /// argument through in order.
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

    /// A `GApplication` id needs two or more dot-separated non-empty
    /// components (reverse-DNS style). A bad id is rejected by GLib at
    /// runtime and would break the app on every start, so the well-formedness
    /// of this constant stays covered by a unit test.
    #[test]
    fn test_app_id_is_well_formed() {
        let components: Vec<&str> = APP_ID.split('.').collect();
        assert!(
            components.len() >= 2 && components.iter().all(|c| !c.is_empty()),
            "GApplication ids need two or more non-empty components; the id is '{}'",
            APP_ID
        );
    }

    /// The desktop entry is named after the application id (`<APP_ID>.desktop`)
    /// so shells and docks attribute the running window to it. If the id
    /// ever changes, the entry file must be renamed with it.
    #[test]
    fn test_desktop_entry_named_after_app_id() {
        let entry = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/share/"))
            .join(format!("{}.desktop", APP_ID));
        assert!(
            entry.exists(),
            "share/{}.desktop must exist for dock attribution; if the app id was renamed, the entry file must move too",
            APP_ID
        );
    }
}
