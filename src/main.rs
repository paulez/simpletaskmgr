use log::info;
use simplelog::*;

use gtk4::prelude::*;

fn main() {
    let log_config = ConfigBuilder::new()
        .add_filter_allow_str("simpletaskmgr")
        .build();
    let _ = SimpleLogger::init(LevelFilter::Debug, log_config);
    info!("Starting simpletaskmgr");

    let app = gtk4::Application::builder()
        .application_id("org.simpletaskmgr.simpletaskmgr")
        .build();
    app.connect_activate(|app| {
        let win = simpletaskmgr::ui::build_window(app);
        win.set_visible(true);
    });
    let _ = app.run();
}
