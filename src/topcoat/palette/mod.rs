//! Private command palette. Route owners register actions and search results
//! through `window.lificPalette.register(owner, registration)`; the returned
//! function unregisters that owner. Registration types live in `api.d.ts`.
//! Load the script after the session and sync bridges.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-palette.js";
pub(crate) const SCRIPT: &str = include_str!("assets/palette.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-palette.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/palette.css");

/// Mount once in private shell chrome; public and account routes omit it.
pub(crate) fn palette(cx: &Cx) -> BoxView<'_> {
    view! { cx => <div class="tc-palette-mount">
        <dialog data-topcoat-palette="" class="tc-palette" aria-label="Jump or act">
            <div class="tc-palette__input-row">
                <span data-palette-mode="" hidden="hidden"></span>
                <input data-palette-input="" type="text" aria-label="Search or run an action"
                    role="combobox" aria-autocomplete="list" aria-expanded="true"
                    aria-controls="tc-palette-results" autocomplete="off" spellcheck="false"
                    placeholder="Jump or act…">
                <button type="button" class="tc-button" data-palette-close="" aria-label="Close search">"Close"</button>
            </div>
            <div id="tc-palette-results" data-palette-results="" role="listbox" aria-label="Results"></div>
            <p data-palette-status="" role="status" aria-live="polite"></p>
            <p data-palette-error="" role="status" aria-live="polite" hidden="hidden"></p>
            <footer>"↑↓ move · Enter open · Ctrl/⌘ Enter new tab"</footer>
        </dialog>
        <dialog data-palette-help="" class="tc-shortcut-help" aria-label="Keyboard shortcuts">
            <header><h2>"Keyboard shortcuts"</h2>
                <button type="button" class="tc-button" data-shortcut-close="" aria-label="Close shortcuts">"Close"</button>
            </header>
            <div data-shortcut-list=""></div>
        </dialog>
    </div>
    }.boxed()
}
