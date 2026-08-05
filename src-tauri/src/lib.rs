mod app;
mod commands;
mod harness;
mod library;
mod runner;
mod store;
mod validate;
mod vault;

use app::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("HALATION_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = AppState::init(app.handle()).map_err(std::io::Error::other)?;

            // Let the webview load files out of the library, and nothing else.
            //
            // `assetProtocol.scope` in tauri.conf.json is a static list, and the
            // library root is chosen at runtime — so it was left empty, which
            // does not mean "allow all", it means **allow nothing**. Every
            // `asset://` URL was blocked and every finished generation rendered
            // as a blank card next to a perfectly good file on disk.
            //
            // Granting the real root here is both narrower than any glob we
            // could have written and correct when the user moves it.
            let root = state.library.root().to_path_buf();
            if let Err(e) = std::fs::create_dir_all(&root) {
                tracing::warn!("could not create the library at {}: {e}", root.display());
            }
            app.asset_protocol_scope().allow_directory(&root, true)?;
            tracing::info!("library: {}", root.display());

            // Anything the last session left running is picked back up here.
            // This is the payoff for keeping job state in Rust rather than in
            // the webview: quitting mid-generation costs the user nothing.
            state.resume();
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::key_states,
            commands::set_key,
            commands::configured_providers,
            commands::local_endpoints,
            commands::is_ready,
            commands::provider_info,
            commands::validate_key,
            commands::import_env,
            commands::list_models,
            commands::list_use_cases,
            commands::detect_gaps,
            commands::models_for_use_case,
            commands::model_capabilities,
            commands::list_presets,
            commands::estimate_cost,
            commands::submit_job,
            commands::list_jobs,
            commands::cancel_job,
            commands::delete_job,
            commands::reveal_result,
            commands::watching_jobs,
            commands::library_root,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Halation");
}
