use helix_event::register_hook;
use helix_view::events::DocumentFocusLost;
use helix_view::handlers::Handlers;

use crate::job::{self};
use crate::ui;

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |_event: &mut DocumentFocusLost<'_>| {
        job::dispatch_blocking(move |_, compositor| {
            // A passphrase prompt belongs to the encrypted file it opens, not
            // to the focused one, and opening that file moves the focus.
            if compositor.find::<ui::Prompt>().is_some()
                && compositor
                    .find_id::<ui::Prompt>(ui::Prompt::SECRET_ID)
                    .is_none()
            {
                compositor.remove_type::<ui::Prompt>();
            }
        });
        Ok(())
    });
}
