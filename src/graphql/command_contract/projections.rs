use serde::Serialize;

use crate::domain_event::{DomainEventBodyContract, DomainEventContract};
use crate::projection::lower::{ProjectionBodyMetadata, ProjectionPortableType};
use crate::{
    DomainEvent, DomainEventBodyKind, DomainEventDescriptor, DomainState, ProjectionEnvelopeField,
    ProjectionEventSelector, ProjectionValue,
};

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
    /// The body property is known to be omitted.
    Absent,
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
    pub(crate) envelope: Option<ProjectionEnvelopeField>,
    #[serde(skip)]
    pub(crate) body_type: Option<ProjectionPortableType>,
    #[serde(skip)]
    pub(crate) body_rust_type: Option<&'static str>,
    #[serde(skip)]
    pub(crate) body_nullable: Option<bool>,
    #[serde(skip)]
    pub(crate) body_always_present: Option<bool>,
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
            envelope: None,
            body_type: None,
            body_rust_type: None,
            body_nullable: None,
            body_always_present: None,
            source,
        });
        self
    }

    /// Bind one non-intrinsic occurrence-envelope field to command provenance.
    #[must_use]
    pub fn envelope(
        mut self,
        field: ProjectionEnvelopeField,
        source: CommandProjectionPreviewSource,
    ) -> Self {
        self.fields.push(CommandProjectionPreviewField {
            body_path: Vec::new(),
            envelope: Some(field),
            body_type: None,
            body_rust_type: None,
            body_nullable: None,
            body_always_present: None,
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
        if preview.selectors.is_empty() {
            self.declaration_errors
                .push("projection preview must bind exactly one emitted event variant".to_owned());
            return;
        }
        if preview.selectors.len() != 1 {
            self.declaration_errors.push(
                "projection preview must bind one exact event variant, not a multi-event set"
                    .to_owned(),
            );
            return;
        }
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
                .sort_by(|left, right| preview_field_key(left).cmp(&preview_field_key(right)));
            for pair in preview.preview.fields.windows(2) {
                if preview_field_key(&pair[0]) == preview_field_key(&pair[1]) {
                    return Err(format!(
                        "typed command `{command}` repeats preview provenance for one event value"
                    ));
                }
            }
            for field in &preview.preview.fields {
                if field.envelope.is_none() {
                    validate_path(command, "emitted body", &field.body_path)?;
                }
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
                    | CommandProjectionPreviewSource::Absent
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
    descriptors: impl IntoIterator<Item = Result<DomainEventDescriptor, String>>,
) -> CommandProjectionEventSet {
    let mut events = CommandProjectionEventSet::default();
    for descriptor in descriptors {
        let descriptor = match descriptor {
            Ok(descriptor) => descriptor,
            Err(error) => {
                events.declaration_errors.push(error);
                continue;
            }
        };
        match ProjectionEventSelector::try_from_descriptor(&descriptor) {
            Ok(selector) => events.selectors.push(selector),
            Err(error) => events.declaration_errors.push(error.to_string()),
        }
    }
    events
}

/// Resolve one exact typed command event descriptor for `events!`.
#[doc(hidden)]
pub fn __command_projection_event_descriptor<E: DomainEventContract>(
) -> Result<DomainEventDescriptor, String> {
    let descriptor = E::descriptor();
    if descriptor.name != E::EVENT_NAME {
        return Err(format!(
            "event contract name `{}` differs from descriptor name `{}`",
            E::EVENT_NAME,
            descriptor.name
        ));
    }
    if descriptor.version != E::EVENT_VERSION {
        return Err(format!(
            "event contract `{}` version {} differs from descriptor version {}",
            E::EVENT_NAME,
            E::EVENT_VERSION,
            descriptor.version
        ));
    }
    Ok(descriptor)
}

/// Build a structured state preview from generated body metadata.
#[doc(hidden)]
pub fn __command_projection_state_preview<E, S>(
    fields: Vec<(&'static str, CommandProjectionPreviewSource)>,
) -> CommandProjectionPreview
where
    E: DomainEventBodyContract<S>,
    S: DomainState + ProjectionBodyMetadata,
{
    let descriptor = __command_projection_event_descriptor::<E>().and_then(|descriptor| {
        let expected = DomainEventDescriptor::state::<S>(E::EVENT_NAME, E::EVENT_VERSION);
        if descriptor != expected || descriptor.body.kind != DomainEventBodyKind::State {
            return Err(format!(
                "state preview event contract `{}` does not exactly describe `{}` state",
                E::EVENT_NAME,
                std::any::type_name::<S>()
            ));
        }
        Ok(descriptor)
    });
    structured_preview::<S>(__command_projection_events([descriptor]), fields)
}

/// Build a structured sparse-event preview from generated body metadata.
#[doc(hidden)]
pub fn __command_projection_event_preview<E, B>(
    fields: Vec<(&'static str, CommandProjectionPreviewSource)>,
) -> CommandProjectionPreview
where
    E: DomainEventBodyContract<B>,
    B: DomainEvent + ProjectionBodyMetadata,
{
    let descriptor = __command_projection_event_descriptor::<E>().and_then(|descriptor| {
        if descriptor != B::DESCRIPTOR || descriptor.body.kind != DomainEventBodyKind::Event {
            return Err(format!(
                "event preview contract `{}` differs from its exact typed body descriptor",
                E::EVENT_NAME
            ));
        }
        Ok(descriptor)
    });
    structured_preview::<B>(__command_projection_events([descriptor]), fields)
}

fn structured_preview<B: ProjectionBodyMetadata>(
    events: CommandProjectionEventSet,
    fields: Vec<(&'static str, CommandProjectionPreviewSource)>,
) -> CommandProjectionPreview {
    let mut preview = CommandProjectionPreview::new().events(events);
    for (rust_name, source) in fields {
        match B::PROJECTION_FIELDS
            .iter()
            .find(|field| field.rust_name == rust_name && field.present)
        {
            Some(field) => preview.fields.push(CommandProjectionPreviewField {
                body_path: vec![field.wire_name.to_owned()],
                envelope: None,
                body_type: Some(field.portable_type),
                body_rust_type: Some(field.rust_type),
                body_nullable: Some(field.nullable),
                body_always_present: Some(field.always_present),
                source,
            }),
            None => preview.declaration_errors.push(format!(
                "state preview references unknown body field `{rust_name}`"
            )),
        }
    }
    preview
}

/// Convert a typed constant into the portable preview value lattice.
#[doc(hidden)]
pub fn __command_projection_preview_constant(
    value: impl Serialize,
) -> CommandProjectionPreviewSource {
    match serde_json::to_value(value)
        .map_err(|error| error.to_string())
        .and_then(|value| ProjectionValue::try_from_json(value).map_err(|error| error.to_string()))
    {
        Ok(value) => CommandProjectionPreviewSource::Constant { value },
        Err(_) => CommandProjectionPreviewSource::Unknown,
    }
}

/// Declare an exact, type-checked outward domain-event set.
#[macro_export]
macro_rules! events {
    ($($event:ty),+ $(,)?) => {
        $crate::graphql::__command_projection_events([
            $($crate::graphql::__command_projection_event_descriptor::<$event>()),+
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
        $event:ty => $state:ty { $($fields:tt)* }
    ) => {{
        $crate::graphql::__command_projection_state_preview::<$event, $state>(
            $crate::__distributed_state_preview_fields!(@collect [] ; $($fields)*)
        )
    }};
}

/// Build partial preview provenance for one exact sparse outward event.
#[macro_export]
macro_rules! event_preview {
    (
        $event:ty => $body:ty { $($fields:tt)* }
    ) => {{
        $crate::graphql::__command_projection_event_preview::<$event, $body>(
            $crate::__distributed_state_preview_fields!(@collect [] ; $($fields)*)
        )
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __distributed_state_preview_fields {
    (@collect [$($out:expr,)*] ; ..unknown $(,)?) => {
        vec![$($out,)*]
    };
    (@collect [$($out:expr,)*] ; ) => {
        vec![$($out,)*]
    };
    (@collect [$($out:expr,)*] ;
        $field:ident : input.$first:ident $(.$rest:ident)*,
        $($tail:tt)*
    ) => {
        $crate::__distributed_state_preview_fields!(
            @collect [
                $($out,)*
                (
                    stringify!($field),
                    $crate::graphql::CommandProjectionPreviewSource::input([
                        stringify!($first) $(, stringify!($rest))*
                    ])
                ),
            ];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ;
        $field:ident : generated.$first:ident $(.$rest:ident)*,
        $($tail:tt)*
    ) => {
        $crate::__distributed_state_preview_fields!(
            @collect [
                $($out,)*
                (
                    stringify!($field),
                    $crate::graphql::CommandProjectionPreviewSource::generated_default([
                        stringify!($first) $(, stringify!($rest))*
                    ])
                ),
            ];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ;
        $field:ident : trusted($name:expr, $codec:expr),
        $($tail:tt)*
    ) => {
        $crate::__distributed_state_preview_fields!(
            @collect [
                $($out,)*
                (
                    stringify!($field),
                    $crate::graphql::CommandProjectionPreviewSource::trusted($name, $codec)
                ),
            ];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ; $field:ident : unknown, $($tail:tt)*) => {
        $crate::__distributed_state_preview_fields!(
            @collect [$($out,)* (stringify!($field), $crate::graphql::CommandProjectionPreviewSource::Unknown),];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ; $field:ident : absent, $($tail:tt)*) => {
        $crate::__distributed_state_preview_fields!(
            @collect [$($out,)* (stringify!($field), $crate::graphql::CommandProjectionPreviewSource::Absent),];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ; $field:ident : null, $($tail:tt)*) => {
        $crate::__distributed_state_preview_fields!(
            @collect [$($out,)* (stringify!($field), $crate::graphql::CommandProjectionPreviewSource::Null),];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ; $field:ident : $constant:path, $($tail:tt)*) => {
        $crate::__distributed_state_preview_fields!(
            @collect [
                $($out,)*
                (
                    stringify!($field),
                    $crate::graphql::__command_projection_preview_constant($constant)
                ),
            ];
            $($tail)*
        )
    };
    (@collect [$($out:expr,)*] ; $field:ident : $constant:literal, $($tail:tt)*) => {
        $crate::__distributed_state_preview_fields!(
            @collect [
                $($out,)*
                (
                    stringify!($field),
                    $crate::graphql::__command_projection_preview_constant($constant)
                ),
            ];
            $($tail)*
        )
    };
}

fn validate_path(command: &str, label: &str, path: &[String]) -> Result<(), String> {
    if path.is_empty() || path.iter().any(|segment| segment.trim().is_empty()) {
        return Err(format!(
            "typed command `{command}` {label} path must contain only non-empty segments"
        ));
    }
    Ok(())
}

fn preview_field_key(field: &CommandProjectionPreviewField) -> (u8, Vec<String>) {
    match field.envelope {
        Some(envelope) => (
            1,
            vec![serde_json::to_string(&envelope)
                .expect("projection envelope field serialization cannot fail")],
        ),
        None => (0, field.body_path.clone()),
    }
}

fn preview_bytes(preview: &CommandProjectionPreview) -> Vec<u8> {
    serde_json::to_vec(preview).expect("projection preview serialization cannot fail")
}
