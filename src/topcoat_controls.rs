//! Shared controls for the opt-in Topcoat frontend.
//!
//! Render these with the request's `Cx` and load [`STYLESHEET`] after the
//! scaffold stylesheet. Set `data-theme` on the document to [`Theme::as_str`]
//! to override the system preference. Callers own form actions and state.
//! Controls return `BoxView` so they compose directly in Topcoat 0.9 `view!`.

use topcoat::{
    context::Cx,
    view::{Attributes, BoxView, View, ViewExt, attributes, view},
};

pub(crate) const STYLESHEET: &str = include_str!("assets/topcoat-controls.css");
pub(crate) const PREFERENCES_SCRIPT: &str = include_str!("assets/topcoat-preferences.js");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preference {
    Theme,
    Accent,
    Density,
    FontScale,
}

impl Preference {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Accent => "accent",
            Self::Density => "density",
            Self::FontScale => "fontScale",
        }
    }

    pub(crate) fn options(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Theme => &[("system", "System"), ("light", "Light"), ("dark", "Dark")],
            Self::Accent => &[
                ("indigo", "Indigo"),
                ("teal", "Teal"),
                ("rose", "Rose"),
                ("amber", "Amber"),
                ("green", "Green"),
                ("violet", "Violet"),
            ],
            Self::Density => &[("comfortable", "Comfortable"), ("compact", "Compact")],
            Self::FontScale => &[("small", "Small"), ("normal", "Normal"), ("large", "Large")],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ButtonVariant {
    Primary,
    #[default]
    Secondary,
    Danger,
}

impl ButtonVariant {
    fn class(self) -> &'static str {
        match self {
            Self::Primary => "tc-button tc-button--primary",
            Self::Secondary => "tc-button",
            Self::Danger => "tc-button tc-button--danger",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    #[default]
    Button,
    Submit,
}

impl ButtonKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Submit => "submit",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Button<'a> {
    pub(crate) label: &'a str,
    pub(crate) variant: ButtonVariant,
    pub(crate) kind: ButtonKind,
    pub(crate) disabled: bool,
    pub(crate) loading: bool,
    /// Caller handlers and supplementary attributes. The helper owns `class`,
    /// `type`, `disabled`, and `aria-busy`; use the typed properties for these.
    pub(crate) attrs: Attributes,
}

impl<'a> Button<'a> {
    /// Defaults to a non-submitting secondary button.
    pub(crate) fn new(label: &'a str) -> Self {
        Self {
            label,
            variant: ButtonVariant::default(),
            kind: ButtonKind::default(),
            disabled: false,
            loading: false,
            attrs: Attributes::new(),
        }
    }
}

pub(crate) fn button<'a>(cx: &'a Cx, props: Button<'a>) -> BoxView<'a> {
    let attrs = attributes! { cx =>
        (props.attrs)
        class=(props.variant.class())
        type=(props.kind.as_str())
        disabled=(props.disabled || props.loading)
        aria-busy=(props.loading.then_some("true"))
    };
    view! { cx =>
        <button (attrs)>
            if props.loading {
                <span class="tc-spinner" aria-hidden="true"></span>
            }
            (props.label)
        </button>
    }
    .boxed()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InputKind {
    #[default]
    Text,
    Email,
    Password,
    Search,
}

impl InputKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Email => "email",
            Self::Password => "password",
            Self::Search => "search",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TextInput<'a> {
    /// Must be unique within the document; also prefixes the error element ID.
    pub(crate) id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) label: &'a str,
    pub(crate) value: &'a str,
    pub(crate) kind: InputKind,
    pub(crate) autocomplete: Option<&'a str>,
    pub(crate) required: bool,
    pub(crate) disabled: bool,
    pub(crate) error: Option<&'a str>,
    /// Caller handlers and supplementary attributes. Generated attributes
    /// for the field's identity, value, type, state, and error remain owned by
    /// the helper; use the typed properties for these. Caller descriptions
    /// remain linked alongside the generated error description.
    pub(crate) attrs: Attributes,
}

impl<'a> TextInput<'a> {
    pub(crate) fn new(id: &'a str, name: &'a str, label: &'a str, value: &'a str) -> Self {
        Self {
            id,
            name,
            label,
            value,
            kind: InputKind::default(),
            autocomplete: None,
            required: false,
            disabled: false,
            error: None,
            attrs: Attributes::new(),
        }
    }
}

fn append_error_description(cx: &Cx, attrs: &mut Attributes, error_id: &str) {
    if let Some(hint) = attrs
        .remove("aria-describedby")
        .filter(|hint| hint.is_present())
    {
        attrs.insert(cx, "aria-describedby", (hint, " ", error_id));
    } else {
        attrs.insert(cx, "aria-describedby", error_id);
    }
}

pub(crate) fn text_input<'a>(cx: &'a Cx, mut props: TextInput<'a>) -> BoxView<'a> {
    let error_id = format!("{}-error", props.id);
    if props.error.is_some() {
        append_error_description(cx, &mut props.attrs, &error_id);
    }
    let attrs = attributes! { cx =>
        (props.attrs)
        class="tc-input"
        id=(props.id)
        name=(props.name)
        type=(props.kind.as_str())
        value=(props.value)
        autocomplete=(props.autocomplete)
        required=(props.required)
        disabled=(props.disabled)
        aria-invalid=(props.error.map(|_| "true"))
    };
    view! { cx =>
        <div class="tc-field">
            <label class="tc-field__label" for=(props.id)>
                (props.label)
                if props.required { <span>" (required)"</span> }
            </label>
            <input (attrs)>
            if let Some(message) = props.error {
                <p class="tc-field__error" id=(error_id) role="alert">(message)</p>
            }
        </div>
    }
    .boxed()
}

#[derive(Debug, Clone)]
pub(crate) struct Select<'a> {
    pub(crate) id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) label: &'a str,
    /// `(value, label)` pairs; include an empty option for an optional field.
    pub(crate) options: &'a [(&'a str, &'a str)],
    pub(crate) selected: &'a str,
    pub(crate) required: bool,
    pub(crate) disabled: bool,
    pub(crate) error: Option<&'a str>,
    /// Caller handlers/supplementary attributes; generated field attributes
    /// win, while caller descriptions remain linked alongside field errors.
    pub(crate) attrs: Attributes,
}

impl<'a> Select<'a> {
    pub(crate) fn new(
        id: &'a str,
        name: &'a str,
        label: &'a str,
        options: &'a [(&'a str, &'a str)],
        selected: &'a str,
    ) -> Self {
        Self {
            id,
            name,
            label,
            options,
            selected,
            required: false,
            disabled: false,
            error: None,
            attrs: Attributes::new(),
        }
    }
}

/// Uses the browser's select keyboard, focus, and submission behavior.
pub(crate) fn select<'a>(cx: &'a Cx, mut props: Select<'a>) -> BoxView<'a> {
    let error_id = format!("{}-error", props.id);
    if props.error.is_some() {
        append_error_description(cx, &mut props.attrs, &error_id);
    }
    let attrs = attributes! { cx =>
        (props.attrs)
        class="tc-input tc-select"
        id=(props.id)
        name=(props.name)
        required=(props.required)
        disabled=(props.disabled)
        aria-invalid=(props.error.map(|_| "true"))
    };
    view! { cx =>
        <div class="tc-field">
            <label class="tc-field__label" for=(props.id)>
                (props.label)
                if props.required { <span>" (required)"</span> }
            </label>
            <select (attrs)>
                for (value, label) in props.options {
                    <option value=(*value) selected=(*value == props.selected)>(*label)</option>
                }
            </select>
            if let Some(message) = props.error {
                <p class="tc-field__error" id=(error_id) role="alert">(message)</p>
            }
        </div>
    }
    .boxed()
}

/// The shared preferences script applies/persists this control in both private
/// and public pages. It requires no authenticated API request.
pub(crate) fn preference_select<'a>(
    cx: &'a Cx,
    id: &'a str,
    label: &'a str,
    preference: Preference,
    selected: &'a str,
) -> BoxView<'a> {
    let mut props = Select::new(
        id,
        preference.as_str(),
        label,
        preference.options(),
        selected,
    );
    props
        .attrs
        .insert(cx, "data-tc-preference", preference.as_str());
    select(cx, props)
}

/// Announces loading without making decorative animation part of the label.
pub(crate) fn loading<'a>(cx: &'a Cx, message: &'a str) -> BoxView<'a> {
    view! { cx =>
        <p class="tc-loading" role="status" aria-live="polite" aria-atomic="true">
            <span class="tc-spinner" aria-hidden="true"></span>
            (message)
        </p>
    }
    .boxed()
}

pub(crate) fn error_message<'a>(cx: &'a Cx, message: &'a str) -> BoxView<'a> {
    view! { cx =>
        <p class="tc-error" role="alert" aria-atomic="true">(message)</p>
    }
    .boxed()
}

/// Opens the matching native dialog modally, preserving browser focus and
/// Escape handling. The ID is an escaped data attribute, never JavaScript.
pub(crate) fn dialog_trigger<'a>(cx: &'a Cx, id: &'a str, label: &'a str) -> BoxView<'a> {
    view! { cx =>
        <button
            class="tc-button"
            type="button"
            data-dialog=(id)
            aria-controls=(id)
            aria-haspopup="dialog"
            onclick="document.getElementById(this.dataset.dialog).showModal()"
        >(label)</button>
    }
    .boxed()
}

/// Starts closed. `dialog_trigger` opens it with `showModal`; the native close
/// form and Escape dismiss it. Use unique IDs and keep body forms separate from
/// the close form so closing never submits a mutation.
pub(crate) fn dialog<'a>(
    cx: &'a Cx,
    id: &'a str,
    title: &'a str,
    body: impl View + 'a,
) -> BoxView<'a> {
    let title_id = format!("{id}-title");
    let body = body.boxed();
    view! { cx =>
        <dialog class="tc-dialog" id=(id) aria-labelledby=(title_id.clone())>
            <header class="tc-dialog__header">
                <h2 id=(title_id)>(title)</h2>
                <form method="dialog">
                    <button class="tc-button" type="submit" aria-label="Close dialog">
                        "Close"
                    </button>
                </form>
            </header>
            <div class="tc-dialog__body">(body)</div>
        </dialog>
    }
    .boxed()
}

pub(crate) fn popover_trigger<'a>(cx: &'a Cx, id: &'a str, label: &'a str) -> BoxView<'a> {
    view! { cx =>
        <button class="tc-button" type="button" popovertarget=(id) aria-controls=(id)
            aria-haspopup="dialog">(label)</button>
    }
    .boxed()
}

/// Native auto popover: light-dismiss, Escape, and tab order are browser-owned.
/// The body contains ordinary links/controls instead of an ARIA menu requiring
/// separate arrow-key navigation.
pub(crate) fn popover<'a>(
    cx: &'a Cx,
    id: &'a str,
    title: &'a str,
    body: impl View + 'a,
) -> BoxView<'a> {
    let title_id = format!("{id}-title");
    let body = body.boxed();
    view! { cx =>
        <div class="tc-popover" id=(id) popover="auto" role="dialog"
            aria-labelledby=(title_id.clone())>
            <header class="tc-dialog__header">
                <h2 id=(title_id)>(title)</h2>
                <button class="tc-button" type="button" popovertarget=(id)
                    popovertargetaction="hide" aria-label="Close popover">"Close"</button>
            </header>
            <div class="tc-dialog__body">(body)</div>
        </div>
    }
    .boxed()
}

/// Callers supply the mutation/retry actions, including backend payloads and
/// event handlers; the confirmation shares native dialog dismissal behavior.
pub(crate) fn confirmation<'a>(
    cx: &'a Cx,
    id: &'a str,
    title: &'a str,
    message: &'a str,
    actions: impl View + 'a,
) -> BoxView<'a> {
    let actions = actions.boxed();
    dialog(
        cx,
        id,
        title,
        view! { cx =>
            <p>(message)</p>
            <div class="tc-actions">(actions)</div>
        },
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ToastKind {
    #[default]
    Info,
    Success,
    Error,
}

pub(crate) fn toast<'a>(cx: &'a Cx, message: &'a str, kind: ToastKind) -> BoxView<'a> {
    let (class, role) = match kind {
        ToastKind::Info => ("tc-toast", "status"),
        ToastKind::Success => ("tc-toast tc-toast--success", "status"),
        ToastKind::Error => ("tc-toast tc-toast--error", "alert"),
    };
    view! { cx =>
        <div class=(class) data-tc-toast="" role=(role) aria-atomic="true">
            <p>(message)</p>
            <button class="tc-button" type="button" aria-label="Dismiss notification"
                onclick="this.closest('[data-tc-toast]').remove()">"Dismiss"</button>
        </div>
    }
    .boxed()
}

pub(crate) fn skeleton<'a>(cx: &'a Cx, message: &'a str) -> BoxView<'a> {
    view! { cx =>
        <div class="tc-skeleton" role="status" aria-live="polite" aria-atomic="true">
            <span class="tc-visually-hidden">(message)</span>
            <span class="tc-skeleton__line" aria-hidden="true"></span>
            <span class="tc-skeleton__line" aria-hidden="true"></span>
            <span class="tc-skeleton__line" aria-hidden="true"></span>
        </div>
    }
    .boxed()
}

pub(crate) fn empty_state<'a>(
    cx: &'a Cx,
    title: &'a str,
    message: &'a str,
    actions: impl View + 'a,
) -> BoxView<'a> {
    let actions = actions.boxed();
    view! { cx =>
        <section class="tc-empty">
            <h2>(title)</h2>
            <p>(message)</p>
            <div class="tc-actions">(actions)</div>
        </section>
    }
    .boxed()
}

pub(crate) fn error_state<'a>(
    cx: &'a Cx,
    title: &'a str,
    message: &'a str,
    actions: impl View + 'a,
) -> BoxView<'a> {
    let actions = actions.boxed();
    view! { cx =>
        <section class="tc-error tc-error-state">
            <div role="alert" aria-atomic="true"><h2>(title)</h2><p>(message)</p></div>
            <div class="tc-actions">(actions)</div>
        </section>
    }
    .boxed()
}

/// Hover/focus exposes the description; the shared preferences script lets
/// Escape dismiss it until the next pointer/focus entry.
pub(crate) fn tooltip<'a>(
    cx: &'a Cx,
    id: &'a str,
    label: &'a str,
    message: &'a str,
) -> BoxView<'a> {
    view! { cx =>
        <span class="tc-tooltip">
            <button class="tc-button" type="button" aria-describedby=(id)>(label)</button>
            <span class="tc-tooltip__content" id=(id) role="tooltip">(message)</span>
        </span>
    }
    .boxed()
}

pub(crate) fn kbd<'a>(cx: &'a Cx, shortcut: &'a str) -> BoxView<'a> {
    view! { cx => <kbd class="tc-kbd">(shortcut)</kbd> }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn render(cx: &Cx, control: impl View) -> String {
        control.single().await.unwrap().render(cx)
    }

    #[tokio::test]
    async fn controls_button_defaults_do_not_submit_forms_and_escape_labels() {
        let cx = Cx::default();
        let html = render(&cx, button(&cx, Button::new("Save <draft> & review"))).await;
        assert!(html.contains("type=\"button\""));
        assert!(html.contains("Save &lt;draft&gt; &amp; review"));
        assert!(!html.contains("disabled"));
        assert!(!html.contains("aria-busy"));
    }

    #[tokio::test]
    async fn controls_loading_and_disabled_buttons_block_repeat_submission() {
        let cx = Cx::default();
        let mut props = Button::new("Save");
        props.kind = ButtonKind::Submit;
        props.variant = ButtonVariant::Primary;
        props.loading = true;
        let html = render(&cx, button(&cx, props.clone())).await;
        assert!(html.contains("type=\"submit\""));
        assert!(html.contains("disabled=\"\""));
        assert!(html.contains("aria-busy=\"true\""));
        assert!(html.contains("aria-hidden=\"true\""));
        assert!(html.contains("tc-button--primary"));

        props.loading = false;
        props.disabled = true;
        props.variant = ButtonVariant::Danger;
        let html = render(&cx, button(&cx, props)).await;
        assert!(html.contains("disabled=\"\""));
        assert!(html.contains("tc-button--danger"));
        assert!(!html.contains("aria-busy"));
    }

    #[tokio::test]
    async fn controls_button_keeps_caller_handlers_and_form_attrs_with_owned_state() {
        let cx = &Cx::default();
        let mut props = Button::new("Save");
        props.kind = ButtonKind::Submit;
        props.variant = ButtonVariant::Primary;
        props.loading = true;
        props.attrs = attributes! { cx =>
            @click="() => console.log('clicked')"
            form="issue-form"
            name="action"
            value="save"
            class="unrelated"
            type="reset"
            disabled=(false)
            aria-busy="false"
        };
        let html = render(cx, button(cx, props.clone())).await;
        for expected in [
            "data-topcoat-on:click=\"() => console.log('clicked')\"",
            "form=\"issue-form\"",
            "name=\"action\"",
            "value=\"save\"",
            "class=\"tc-button tc-button--primary\"",
            "type=\"submit\"",
            "disabled=\"\"",
            "aria-busy=\"true\"",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
        assert!(!html.contains("unrelated"));
        assert!(!html.contains("type=\"reset\""));
        assert_eq!(html.matches(" disabled=").count(), 1);

        props.loading = false;
        props.kind = ButtonKind::Button;
        props.attrs.insert(cx, "disabled", true);
        props.attrs.insert(cx, "aria-busy", "true");
        let html = render(cx, button(cx, props)).await;
        assert!(html.contains("type=\"button\""));
        assert!(html.contains("data-topcoat-on:click="));
        assert!(!html.contains(" disabled="));
        assert!(!html.contains("aria-busy="));
    }

    #[tokio::test]
    async fn controls_input_links_its_label_and_error_and_preserves_form_values() {
        let cx = Cx::default();
        let mut props = TextInput::new("email", "email", "Email", "a\"b@example.com");
        props.kind = InputKind::Email;
        props.autocomplete = Some("email");
        props.required = true;
        props.error = Some("Use a valid email <address>");
        let html = render(&cx, text_input(&cx, props)).await;
        for expected in [
            "for=\"email\"",
            "id=\"email\"",
            "name=\"email\"",
            "type=\"email\"",
            "autocomplete=\"email\"",
            "value=\"a&quot;b@example.com\"",
            "required=\"\"",
            "aria-invalid=\"true\"",
            "aria-describedby=\"email-error\"",
            "id=\"email-error\"",
            "role=\"alert\"",
            "Use a valid email &lt;address&gt;",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
    }

    #[tokio::test]
    async fn controls_valid_input_omits_error_relationships_and_honors_disabled() {
        let cx = Cx::default();
        let mut props = TextInput::new("password", "password", "Password", "");
        props.kind = InputKind::Password;
        props.disabled = true;
        let html = render(&cx, text_input(&cx, props)).await;
        assert!(html.contains("type=\"password\""));
        assert!(html.contains("disabled=\"\""));
        assert!(!html.contains("aria-invalid"));
        assert!(!html.contains("aria-describedby"));
        assert!(!html.contains("role=\"alert\""));
        assert!(!html.contains("required="));
        assert_eq!(InputKind::Search.as_str(), "search");
    }

    #[tokio::test]
    async fn controls_input_keeps_caller_handlers_without_losing_identity_and_error() {
        let cx = &Cx::default();
        let mut props = TextInput::new("title", "title", "Title", "Keep this value");
        props.required = true;
        props.error = Some("A title is required");
        props.attrs = attributes! { cx =>
            @input="(event) => console.log(event.target.value)"
            maxlength="200"
            data-field="issue-title"
            class="unrelated"
            id="other"
            name="other"
            value="wrong"
            type="hidden"
            required=(false)
            disabled=(true)
            aria-invalid="false"
            aria-describedby="title-hint"
        };
        let html = render(cx, text_input(cx, props.clone())).await;
        for expected in [
            "data-topcoat-on:input=\"(event) => console.log(event.target.value)\"",
            "maxlength=\"200\"",
            "data-field=\"issue-title\"",
            "class=\"tc-input\"",
            "id=\"title\"",
            "name=\"title\"",
            "value=\"Keep this value\"",
            "type=\"text\"",
            "required=\"\"",
            "aria-invalid=\"true\"",
            "aria-describedby=\"title-hint title-error\"",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
        assert!(!html.contains("unrelated"));
        assert!(!html.contains(" disabled="));
        assert!(!html.contains("type=\"hidden\""));
        assert!(!html.contains("other-error"));
        assert_eq!(html.matches(" aria-describedby=").count(), 1);

        props.error = None;
        let html = render(cx, text_input(cx, props)).await;
        assert!(html.contains("data-topcoat-on:input="));
        assert!(!html.contains("aria-invalid="));
        assert!(html.contains("aria-describedby=\"title-hint\""));
    }

    #[tokio::test]
    async fn controls_status_and_error_messages_use_separate_announcement_roles() {
        let cx = Cx::default();
        let html = render(&cx, loading(&cx, "Loading issues")).await;
        assert!(html.contains("role=\"status\""));
        assert!(html.contains("aria-live=\"polite\""));
        assert!(html.contains("aria-atomic=\"true\""));
        assert!(html.contains("aria-hidden=\"true\""));
        assert!(html.contains("Loading issues"));

        let html = render(&cx, error_message(&cx, "Request failed <retry>")).await;
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("Request failed &lt;retry&gt;"));
    }

    #[tokio::test]
    async fn controls_dialog_is_named_closed_and_uses_native_open_and_close() {
        let cx = Cx::default();
        let html = render(
            &cx,
            dialog(
                &cx,
                "confirm",
                "Delete <issue>",
                view! { cx => <p>"Confirm?"</p> },
            ),
        )
        .await;
        assert!(html.contains("<dialog"));
        assert!(html.contains("aria-labelledby=\"confirm-title\""));
        assert!(html.contains("id=\"confirm-title\""));
        assert!(html.contains("Delete &lt;issue&gt;"));
        assert!(html.contains("method=\"dialog\""));
        assert!(html.contains("aria-label=\"Close dialog\""));
        assert!(!html.contains(" open"));
        assert!(html.contains("<p>Confirm?</p>"));

        let html = render(&cx, dialog_trigger(&cx, "confirm", "Delete issue")).await;
        assert!(html.contains("data-dialog=\"confirm\""));
        assert!(html.contains("aria-controls=\"confirm\""));
        assert!(html.contains("aria-haspopup=\"dialog\""));
        assert!(html.contains("showModal()"));
        assert!(html.contains("type=\"button\""));
    }

    #[tokio::test]
    async fn controls_dialog_ids_are_escaped_as_data_and_never_interpolated_into_script() {
        let cx = Cx::default();
        let html = render(&cx, dialog_trigger(&cx, "confirm\";alert(1)//", "Open")).await;
        assert!(html.contains("data-dialog=\"confirm&quot;;alert(1)//\""));
        assert!(
            html.contains("onclick=\"document.getElementById(this.dataset.dialog).showModal()\"")
        );
    }

    #[tokio::test]
    async fn controls_compose_inside_views_and_accept_nested_dialog_content() {
        let cx = &Cx::default();
        let controls = view! { cx =>
            <section>
                (button(cx, Button::new("Save")))
                (text_input(cx, TextInput::new("title", "title", "Title", "Draft")))
                (loading(cx, "Loading issues"))
                (error_message(cx, "Try again"))
                (dialog_trigger(cx, "confirm", "Review"))
                (dialog(cx, "confirm", "Review issue", button(cx, Button::new("Keep"))))
            </section>
        };
        let html = render(cx, controls).await;
        for expected in [
            "<section>",
            "Save</button>",
            "value=\"Draft\"",
            "Loading issues",
            "Try again",
            "data-dialog=\"confirm\"",
            "Review issue</h2>",
            "Keep</button>",
            "</section>",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
    }

    #[tokio::test]
    async fn controls_select_preserves_native_form_state_and_validation_relationships() {
        let cx = &Cx::default();
        let options = [
            ("", "Choose a status"),
            ("todo", "To do"),
            ("done", "Done <ready>"),
        ];
        let mut props = Select::new("status", "status", "Status", &options, "done");
        props.required = true;
        props.disabled = true;
        props.error = Some("Choose an allowed status");
        props.attrs = attributes! { cx =>
            @change="(event) => console.log(event.target.value)"
            class="wrong"
            name="wrong"
            required=(false)
            disabled=(false)
            aria-describedby="status-hint"
        };
        let html = render(cx, select(cx, props.clone())).await;
        for expected in [
            "<select ",
            "for=\"status\"",
            "id=\"status\"",
            "name=\"status\"",
            "required=\"\"",
            "disabled=\"\"",
            "aria-invalid=\"true\"",
            "aria-describedby=\"status-hint status-error\"",
            "id=\"status-error\"",
            "data-topcoat-on:change=",
            "value=\"done\" selected=\"\"",
            "Done &lt;ready&gt;",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
        assert_eq!(html.matches(" selected=").count(), 1);
        assert!(!html.contains("name=\"wrong\""));

        props.required = false;
        props.disabled = false;
        props.error = None;
        let html = render(cx, select(cx, props)).await;
        assert!(!html.contains("required="));
        assert!(!html.contains("disabled="));
        assert!(!html.contains("aria-invalid="));
        assert!(html.contains("aria-describedby=\"status-hint\""));
    }

    #[tokio::test]
    async fn controls_preferences_bind_selects_to_the_shared_script_and_tokens() {
        let cx = &Cx::default();
        for preference in [
            Preference::Theme,
            Preference::Accent,
            Preference::Density,
            Preference::FontScale,
        ] {
            let selected = preference.options()[0].0;
            let html = render(
                cx,
                preference_select(cx, preference.as_str(), "Preference", preference, selected),
            )
            .await;
            assert!(html.contains(&format!("data-tc-preference=\"{}\"", preference.as_str())));
            for (value, _) in preference.options() {
                assert!(html.contains(&format!("value=\"{value}\"")));
                assert!(PREFERENCES_SCRIPT.contains(&format!("\"{value}\"")));
                let attribute = match preference {
                    Preference::Theme => "theme",
                    Preference::Accent => "accent",
                    Preference::Density => "density",
                    Preference::FontScale => "font-scale",
                };
                if preference != Preference::Theme || *value != "system" {
                    assert!(STYLESHEET.contains(&format!("[data-{attribute}=\"{value}\"]")));
                }
            }
        }
    }

    #[tokio::test]
    async fn controls_popover_uses_native_toggle_dismissal_and_an_accessible_name() {
        let cx = &Cx::default();
        let html = render(
            cx,
            view! { cx =>
                (popover_trigger(cx, "filters", "Filter issues"))
                (popover(cx, "filters", "Issue filters", button(cx, Button::new("Apply"))))
            },
        )
        .await;
        for expected in [
            "popovertarget=\"filters\"",
            "popover=\"auto\"",
            "role=\"dialog\"",
            "aria-labelledby=\"filters-title\"",
            "id=\"filters-title\"",
            "popovertargetaction=\"hide\"",
            "aria-label=\"Close popover\"",
            "Apply</button>",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
        assert!(!html.contains("role=\"menu\""));
    }

    #[tokio::test]
    async fn controls_confirmation_keeps_caller_actions_and_native_modal_dismissal() {
        let cx = &Cx::default();
        let mut action = Button::new("Delete");
        action.variant = ButtonVariant::Danger;
        action.attrs = attributes! { cx => @click="() => console.log('confirmed')" };
        let html = render(
            cx,
            confirmation(
                cx,
                "delete",
                "Delete issue?",
                "Delete <draft>?",
                button(cx, action),
            ),
        )
        .await;
        assert!(html.contains("<dialog"));
        assert!(html.contains("aria-labelledby=\"delete-title\""));
        assert!(html.contains("Delete &lt;draft&gt;?"));
        assert!(html.contains("tc-button--danger"));
        assert!(html.contains("data-topcoat-on:click=\"() => console.log('confirmed')\""));
        assert!(html.contains("method=\"dialog\""));
    }

    #[tokio::test]
    async fn controls_toasts_announce_severity_and_offer_explicit_dismissal() {
        let cx = &Cx::default();
        for (kind, role) in [
            (ToastKind::Info, "status"),
            (ToastKind::Success, "status"),
            (ToastKind::Error, "alert"),
        ] {
            let html = render(cx, toast(cx, "Saved <issue>", kind)).await;
            assert!(html.contains(&format!("role=\"{role}\"")));
            assert!(html.contains("aria-atomic=\"true\""));
            assert!(html.contains("Saved &lt;issue&gt;"));
            assert!(html.contains("aria-label=\"Dismiss notification\""));
            assert!(html.contains("this.closest('[data-tc-toast]').remove()"));
        }
    }

    #[tokio::test]
    async fn controls_skeleton_empty_and_error_states_keep_labels_and_caller_actions() {
        let cx = &Cx::default();
        let html = render(cx, skeleton(cx, "Loading issues")).await;
        assert!(html.contains("role=\"status\""));
        assert!(html.contains("Loading issues"));
        assert_eq!(html.matches("aria-hidden=\"true\"").count(), 3);

        let html = render(
            cx,
            empty_state(
                cx,
                "No issues",
                "Create the first issue.",
                button(cx, Button::new("Create issue")),
            ),
        )
        .await;
        assert!(html.contains("No issues</h2>"));
        assert!(html.contains("Create the first issue."));
        assert!(html.contains("Create issue</button>"));

        let html = render(
            cx,
            error_state(
                cx,
                "Issues unavailable",
                "Try again.",
                button(cx, Button::new("Retry")),
            ),
        )
        .await;
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("Issues unavailable</h2>"));
        assert!(html.contains("Retry</button>"));
    }

    #[tokio::test]
    async fn controls_tooltip_and_keyboard_hint_link_descriptions_and_escape_content() {
        let cx = &Cx::default();
        let html = render(
            cx,
            view! { cx =>
                (tooltip(cx, "shortcut-help", "Help", "Press <Enter> to confirm."))
                (kbd(cx, "Ctrl + <Enter>"))
            },
        )
        .await;
        assert!(html.contains("aria-describedby=\"shortcut-help\""));
        assert!(html.contains("id=\"shortcut-help\" role=\"tooltip\""));
        assert!(html.contains("Press &lt;Enter&gt; to confirm."));
        assert!(html.contains("<kbd class=\"tc-kbd\">Ctrl + &lt;Enter&gt;</kbd>"));
        assert!(STYLESHEET.contains(":focus-within"));
        assert!(STYLESHEET.contains("data-dismissed"));
        assert!(PREFERENCES_SCRIPT.contains("Escape"));
    }

    #[tokio::test]
    async fn controls_input_preserves_hint_descriptions_with_and_without_errors() {
        let cx = &Cx::default();
        for error in [None, Some("A title is required")] {
            let mut props = TextInput::new("title", "title", "Title", "");
            props.error = error;
            props.attrs = attributes! { cx => aria-describedby="title-hint formatting-hint" };
            let html = render(cx, text_input(cx, props)).await;
            let expected = if error.is_some() {
                "aria-describedby=\"title-hint formatting-hint title-error\""
            } else {
                "aria-describedby=\"title-hint formatting-hint\""
            };
            assert!(html.contains(expected), "missing {expected}: {html}");
            assert_eq!(html.matches(" aria-describedby=").count(), 1);
        }
    }

    #[tokio::test]
    async fn controls_select_preserves_hint_descriptions_with_and_without_errors() {
        let cx = &Cx::default();
        let options = [("todo", "To do")];
        for error in [None, Some("Choose a status")] {
            let mut props = Select::new("status", "status", "Status", &options, "todo");
            props.error = error;
            props
                .attrs
                .insert(cx, "aria-describedby", "status-hint &details");
            let html = render(cx, select(cx, props)).await;
            let expected = if error.is_some() {
                "aria-describedby=\"status-hint &amp;details status-error\""
            } else {
                "aria-describedby=\"status-hint &amp;details\""
            };
            assert!(html.contains(expected), "missing {expected}: {html}");
            assert_eq!(html.matches(" aria-describedby=").count(), 1);
        }
    }

    #[tokio::test]
    #[ignore = "requires devenv --profile e2e with repository Playwright/Chromium"]
    async fn controls_tooltip_stays_inside_viewport_edges_with_enlarged_text() {
        let cx = &Cx::default();
        let content = render(
            cx,
            tooltip(
                cx,
                "edge-help",
                "Help",
                "Save with Enter, or press Escape to cancel your changes.",
            ),
        )
        .await;
        let html = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><style>{STYLESHEET}\nbody {{ margin: 0; line-height: 1.5; }} #fixture {{ position: fixed; }}</style></head><body><div id=\"fixture\">{content}</div></body></html>"
        );
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(fixture.path().join("fixture.html"), html).unwrap();
        std::fs::write(fixture.path().join("preferences.js"), PREFERENCES_SCRIPT).unwrap();
        let playwright = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("e2e/node_modules/playwright/index.mjs");
        let script = r#"
            import assert from 'node:assert/strict';
            import { readFileSync } from 'node:fs';
            import { join } from 'node:path';
            const { chromium } = await import(process.env.LIFIC_PLAYWRIGHT_MODULE);
            const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
            try {
                const page = await browser.newPage();
                page.setDefaultTimeout(5000);
                const failures = [];
                page.on('pageerror', error => failures.push(error.message));
                await page.setContent(readFileSync(join(process.env.LIFIC_TOOLTIP_FIXTURE, 'fixture.html'), 'utf8'));
                await page.addScriptTag({ type: 'module', content: readFileSync(join(process.env.LIFIC_TOOLTIP_FIXTURE, 'preferences.js'), 'utf8') });
                const bounds = async () => page.locator('[role=tooltip]').evaluate(element => {
                    const rect = element.getBoundingClientRect();
                    return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, visible: getComputedStyle(element).visibility };
                });
                for (const viewport of [{ width: 320, height: 300 }, { width: 1280, height: 720 }]) {
                    await page.setViewportSize(viewport);
                    for (const scale of ['normal', 'large', '200%']) {
                        await page.evaluate(scale => {
                            document.documentElement.dataset.fontScale = scale === '200%' ? 'normal' : scale;
                            document.documentElement.style.fontSize = scale === '200%' ? scale : '';
                        }, scale);
                        for (const corner of ['top-left', 'top-right', 'bottom-left', 'bottom-right']) {
                            await page.locator('.tc-tooltip button').evaluate(element => element.blur());
                            await page.evaluate(corner => {
                                const fixture = document.querySelector('#fixture');
                                const rect = fixture.getBoundingClientRect();
                                fixture.style.left = `${corner.endsWith('right') ? innerWidth - rect.width : 0}px`;
                                fixture.style.top = `${corner.startsWith('bottom') ? innerHeight - rect.height : 0}px`;
                            }, corner);
                            await page.locator('.tc-tooltip button').focus();
                            const rect = await bounds();
                            assert.equal(rect.visible, 'visible');
                            assert(rect.left >= -0.5 && rect.top >= -0.5 && rect.right <= viewport.width + 0.5 && rect.bottom <= viewport.height + 0.5, `${JSON.stringify({ viewport, scale, corner, rect })}`);
                            const trigger = await page.locator('.tc-tooltip button').boundingBox();
                            if (corner.startsWith('top')) assert(rect.top >= trigger.y + trigger.height - 0.5, 'top-edge tooltip should flip below');
                            else assert(rect.bottom <= trigger.y + 0.5, 'bottom-edge tooltip should stay above');
                        }
                    }
                }
                await page.evaluate(() => { document.documentElement.style.fontSize = ''; document.documentElement.dataset.fontScale = 'normal'; document.querySelector('#fixture').style.cssText = 'position:fixed;left:250px;top:80px'; });
                await page.setViewportSize({ width: 320, height: 300 });
                await page.locator('.tc-tooltip button').focus();
                await page.waitForFunction(() => {
                    const rect = document.querySelector('[role=tooltip]').getBoundingClientRect();
                    return rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth + 0.5 && rect.bottom <= innerHeight + 0.5;
                });
                const resized = await bounds();
                assert(resized.left >= 0 && resized.top >= 0 && resized.right <= 320.5 && resized.bottom <= 300.5, 'resize must reposition the visible tooltip');
                await page.keyboard.press('Escape');
                assert.equal((await bounds()).visible, 'hidden');
                assert.deepEqual(failures, []);
                console.log('tooltip browser edges, enlarged text, resize and dismissal passed');
            } finally { await browser.close(); }
        "#;
        let output = std::process::Command::new("bun")
            .args(["--eval", script])
            .env("LIFIC_PLAYWRIGHT_MODULE", playwright)
            .env("LIFIC_TOOLTIP_FIXTURE", fixture.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn controls_theme_values_match_explicit_and_system_stylesheet_selectors() {
        assert_eq!(Theme::default(), Theme::System);
        assert_eq!(Theme::System.as_str(), "system");
        for theme in [Theme::Light, Theme::Dark] {
            assert!(STYLESHEET.contains(&format!("[data-theme=\"{}\"]", theme.as_str())));
        }
        assert!(STYLESHEET.contains("prefers-color-scheme: dark"));
        assert!(STYLESHEET.contains("prefers-reduced-motion: reduce"));
        assert!(STYLESHEET.contains(":focus-visible"));
    }
}
