use serde::Serialize;

use crate::{DomainEventDescriptor, ProjectionEventSelector, ProjectionValue};

/// One preview-time source for an emitted domain-event body field.
///
/// The source is retained only in the server-side command contract. Client
/// manifests lower body paths to program-scoped opaque slots and never expose
/// the outward event schema's private source paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandProjectionPreviewSource {
    /// Read a value from canonical GraphQL command input.
    InputPath { path: Vec<String> },
    /// Read a value generated into canonical command input before dispatch.
    GeneratedDefaultPath { path: Vec<String> },
    /// Read a framework-authenticated scoped preset by public descriptor.
    TrustedPreset { name: String, codec: String },
    /// Use a portable typed constant.
    Constant { value: ProjectionValue },
    /// Use explicit null.
    Null,
    /// Preserve the projection program's explicit unset assignment.
    Unset,
    /// This one field cannot be predicted; other fields remain usable.
    Unknown,
    /// Deliberately non-portable server-only source.
    ///
    /// Registration rejects this variant. It exists as a fail-closed sentinel
    /// for generated declarations, not as a client escape hatch.
    ServerOnly,
}

impl CommandProjectionPreviewSource {
    /// Construct a canonical input-path source.
    pub fn input(path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::InputPath {
            path: path.into_iter().map(Into::into).collect(),
        }
    }

    /// Construct a generated-default path source.
    pub fn generated_default(path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::GeneratedDefaultPath {
            path: path.into_iter().map(Into::into).collect(),
        }
    }

    /// Construct a trusted scoped-preset source without retaining its value.
    pub fn trusted(name: impl Into<String>, codec: impl Into<String>) -> Self {
        Self::TrustedPreset {
            name: name.into(),
            codec: codec.into(),
        }
    }

    /// Construct a portable constant source.
    pub fn constant(value: ProjectionValue) -> Self {
        Self::Constant { value }
    }
}

/// One emitted-body path and its preview provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommandProjectionPreviewField {
    pub(crate) body_path: Vec<String>,
    pub(crate) source: CommandProjectionPreviewSource,
}

/// Partial, field-by-field preview for one exact emitted event set.
///
/// Missing fields are unknown by definition. Unknown fields do not poison
/// known fields: manifest lowering can still produce a safe partial patch and
/// attach narrow recovery for the unresolved remainder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CommandProjectionPreview {
    pub(crate) selectors: Vec<ProjectionEventSelector>,
    pub(crate) declaration_errors: Vec<String>,
    pub(crate) fields: Vec<CommandProjectionPreviewField>,
}

impl CommandProjectionPreview {
    /// Begin an empty, intentionally partial preview.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind this preview to the exact outward event-set value also passed to
    /// [`crate::graphql::TypedCommand::emits`].
    #[must_use]
    pub fn events(mut self, events: CommandProjectionEventSet) -> Self {
        self.selectors = events.selectors;
        self.declaration_errors = events.declaration_errors;
        self
    }

    /// Bind one emitted-event body path to preview provenance.
    #[must_use]
    pub fn field(
        mut self,
        body_path: impl IntoIterator<Item = impl Into<String>>,
        source: CommandProjectionPreviewSource,
    ) -> Self {
        self.fields.push(CommandProjectionPreviewField {
            body_path: body_path.into_iter().map(Into::into).collect(),
            source,
        });
        self
    }
}

/// Preview declaration for one exact emitted event selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CommandProjectionEventPreview {
    pub selector: ProjectionEventSelector,
    pub preview: CommandProjectionPreview,
}

/// Exact outward events a command may emit, independent of any projector.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct CommandProjectionEvents {
    pub selectors: Vec<ProjectionEventSelector>,
    pub previews: Vec<CommandProjectionEventPreview>,
    pub declaration_errors: Vec<String>,
}

impl CommandProjectionEvents {
    pub(crate) fn add_event_set(&mut self, events: CommandProjectionEventSet) {
        self.selectors.extend(events.selectors);
        self.declaration_errors.extend(events.declaration_errors);
    }

    pub(crate) fn add_preview(&mut self, preview: CommandProjectionPreview) {
        self.declaration_errors
            .extend(preview.declaration_errors.clone());
        self.previews.extend(
            preview
                .selectors
                .iter()
                .cloned()
                .into_iter()
                .map(|selector| CommandProjectionEventPreview {
                    selector,
                    preview: preview.clone(),
                }),
        );
    }

    pub(crate) fn canonicalize_and_validate(&mut self, command: &str) -> Result<(), String> {
        if let Some(error) = self.declaration_errors.first() {
            return Err(format!(
                "typed command `{command}` has an invalid domain-event declaration: {error}"
            ));
        }
        self.selectors
            .sort_by(ProjectionEventSelector::canonical_cmp);
        if self.selectors.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(format!(
                "typed command `{command}` repeats an exact emitted domain event selector"
            ));
        }
        for pair in self.selectors.windows(2) {
            if pair[0].event_name() == pair[1].event_name()
                && pair[0].event_version() == pair[1].event_version()
                && pair[0] != pair[1]
            {
                return Err(format!(
                    "typed command `{command}` declares conflicting schemas for domain event `{}` v{}",
                    pair[0].event_name(),
                    pair[0].event_version()
                ));
            }
        }
        self.previews.sort_by(|left, right| {
            left.selector
                .canonical_cmp(&right.selector)
                .then_with(|| preview_bytes(&left.preview).cmp(&preview_bytes(&right.preview)))
        });
        for preview in &mut self.previews {
            if self
                .selectors
                .binary_search_by(|selector| selector.canonical_cmp(&preview.selector))
                .is_err()
            {
                return Err(format!(
                    "typed command `{command}` declares preview provenance outside its exact emitted event set"
                ));
            }
            preview
                .preview
                .fields
                .sort_by(|left, right| left.body_path.cmp(&right.body_path));
            for pair in preview.preview.fields.windows(2) {
                if pair[0].body_path == pair[1].body_path {
                    return Err(format!(
                        "typed command `{command}` repeats preview provenance for one emitted body path"
                    ));
                }
            }
            for field in &preview.preview.fields {
                validate_path(command, "emitted body", &field.body_path)?;
                match &field.source {
                    CommandProjectionPreviewSource::InputPath { path }
                    | CommandProjectionPreviewSource::GeneratedDefaultPath { path } => {
                        validate_path(command, "preview input", path)?;
                    }
                    CommandProjectionPreviewSource::TrustedPreset { name, codec } => {
                        if name.trim().is_empty() || codec.trim().is_empty() {
                            return Err(format!(
                                "typed command `{command}` preview trusted preset name and codec must not be empty"
                            ));
                        }
                    }
                    CommandProjectionPreviewSource::ServerOnly => {
                        return Err(format!(
                            "typed command `{command}` cannot expose server-only preview provenance"
                        ));
                    }
                    CommandProjectionPreviewSource::Constant { .. }
                    | CommandProjectionPreviewSource::Null
                    | CommandProjectionPreviewSource::Unset
                    | CommandProjectionPreviewSource::Unknown => {}
                }
            }
        }
        for pair in self.previews.windows(2) {
            if pair[0].selector == pair[1].selector {
                return Err(format!(
                    "typed command `{command}` repeats preview provenance for one exact emitted event"
                ));
            }
        }
        Ok(())
    }
}

/// Sealed exact event-set value produced by [`events!`](crate::events).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandProjectionEventSet {
    selectors: Vec<ProjectionEventSelector>,
    declaration_errors: Vec<String>,
}

/// Build the sealed command event-set value used by `events!`.
#[doc(hidden)]
pub fn __command_projection_events(
    descriptors: impl IntoIterator<Item = DomainEventDescriptor>,
) -> CommandProjectionEventSet {
    let mut events = CommandProjectionEventSet::default();
    for descriptor in descriptors {
        match ProjectionEventSelector::try_from_descriptor(&descriptor) {
            Ok(selector) => events.selectors.push(selector),
            Err(error) => events.declaration_errors.push(error.to_string()),
        }
    }
    events
}

/// Declare an exact, type-checked outward domain-event set.
#[macro_export]
macro_rules! events {
    ($($event:ty),+ $(,)?) => {
        $crate::graphql::__command_projection_events([
            $(<$event as $crate::DomainEvent>::DESCRIPTOR.clone()),+
        ])
    };
}

/// Build partial preview provenance for an exact outward event set.
///
/// Body paths remain server-only. Manifest lowering replaces body-path leaves
/// in authoritative projection expressions with program-scoped opaque slots.
#[macro_export]
macro_rules! state_preview {
    (
        events: $events:expr,
        fields: { $($body_path:expr => $source:expr),* $(,)? }
    ) => {{
        let preview = $crate::graphql::CommandProjectionPreview::new().events($events);
        $(let preview = preview.field($body_path, $source);)*
        preview
    }};
}

fn validate_path(command: &str, label: &str, path: &[String]) -> Result<(), String> {
    if path.is_empty() || path.iter().any(|segment| segment.trim().is_empty()) {
        return Err(format!(
            "typed command `{command}` {label} path must contain only non-empty segments"
        ));
    }
    Ok(())
}

fn preview_bytes(preview: &CommandProjectionPreview) -> Vec<u8> {
    serde_json::to_vec(preview).expect("projection preview serialization cannot fail")
}
