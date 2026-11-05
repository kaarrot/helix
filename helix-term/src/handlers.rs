use std::sync::Arc;

use arc_swap::ArcSwap;
use diagnostics::PullAllDocumentsDiagnosticHandler;
use helix_event::AsyncHook;

use crate::config::Config;
use crate::events;
use crate::handlers::auto_save::AutoSaveHandler;
use crate::handlers::diagnostics::PullDiagnosticsHandler;
use crate::handlers::signature_help::SignatureHelpHandler;

pub use helix_view::handlers::{word_index, Handlers};

use self::document_colors::DocumentColorsHandler;

mod auto_save;
pub mod completion;
pub mod diagnostics;
mod document_colors;
#[cfg(feature = "pgp")]
mod pgp;
mod prompt;
mod signature_help;
mod snippet;

pub fn setup(config: Arc<ArcSwap<Config>>) -> Handlers {
    // Match the registry's lifetime: process-wide normally, per Tokio runtime
    // in integration tests. Multiple editors in one registry share its guard.
    helix_event::runtime_local! {
        static EVENTS: std::sync::Once = std::sync::Once::new();
    }
    EVENTS.call_once(events::register);

    let event_tx = completion::CompletionHandler::new(config).spawn();
    let signature_hints = SignatureHelpHandler::new().spawn();
    let auto_save = AutoSaveHandler::new().spawn();
    let document_colors = DocumentColorsHandler::default().spawn();
    let word_index = word_index::Handler::spawn();
    let pull_diagnostics = PullDiagnosticsHandler::default().spawn();
    let pull_all_documents_diagnostics = PullAllDocumentsDiagnosticHandler::default().spawn();

    let handlers = Handlers {
        completions: helix_view::handlers::completion::CompletionHandler::new(event_tx),
        signature_hints,
        auto_save,
        document_colors,
        word_index,
        pull_diagnostics,
        pull_all_documents_diagnostics,
    };

    helix_view::handlers::register_hooks(&handlers);
    completion::register_hooks(&handlers);
    signature_help::register_hooks(&handlers);
    auto_save::register_hooks(&handlers);
    diagnostics::register_hooks(&handlers);
    snippet::register_hooks(&handlers);
    document_colors::register_hooks(&handlers);
    prompt::register_hooks(&handlers);
    #[cfg(feature = "pgp")]
    pgp::register_hooks(&handlers);
    handlers
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    #[test]
    fn regression_applications_register_events_in_each_runtime() {
        for _ in 0..2 {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let mut config = crate::config::Config::default();
                config.editor.lsp.enable = false;
                let mut app = crate::application::Application::new(
                    crate::args::Args::default(),
                    config,
                    helix_core::syntax::Loader::default(),
                )
                .unwrap();
                assert!(app.close().await.is_empty());
            });
        }
    }
}
