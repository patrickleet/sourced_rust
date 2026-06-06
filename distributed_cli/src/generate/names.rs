//! Name / message normalization and validation — the portable spec-value rules
//! this crate owns (kebab/pascal/ident casing, dedup, keyword avoidance).

use std::collections::BTreeSet;

use crate::ScaffoldError;

/// Derived names for the generated service crate.
pub(crate) struct ScaffoldNames {
    pub(crate) package_name: String,
    pub(crate) crate_ident: String,
    pub(crate) command_name: String,
}

impl ScaffoldNames {
    pub(crate) fn new(input: &str) -> Result<Self, ScaffoldError> {
        let package_name = to_kebab_case(input);
        if package_name.is_empty() {
            return Err(ScaffoldError::new(
                "service name must contain at least one ASCII letter or digit",
            ));
        }
        let crate_ident = package_name.replace('-', "_");
        if !is_rust_ident(&crate_ident) {
            return Err(ScaffoldError::new(format!(
                "service name `{input}` yields the invalid Rust crate identifier `{crate_ident}`; \
                 start the name with a letter and avoid Rust keywords"
            )));
        }
        let command_name = format!("{crate_ident}.create");
        Ok(Self {
            package_name,
            crate_ident,
            command_name,
        })
    }
}

/// A scaffolded aggregate model and its derived identifiers.
#[derive(Clone, Debug)]
pub(crate) struct ModelScaffold {
    pub(crate) name: String,
    pub(crate) message_prefix: String,
    pub(crate) module_ident: String,
    pub(crate) type_ident: String,
    pub(crate) view_ident: String,
    pub(crate) table_name: String,
    pub(crate) command_broker: String,
    pub(crate) event_broker: String,
}

impl ModelScaffold {
    pub(crate) fn new(raw_name: &str) -> Result<Self, ScaffoldError> {
        let name = to_kebab_case(raw_name);
        if name.is_empty() {
            return Err(ScaffoldError::new(
                "model name must contain at least one ASCII letter or digit",
            ));
        }
        let ident = name.replace('-', "_");
        if !is_rust_ident(&ident) {
            return Err(ScaffoldError::new(format!(
                "model name `{raw_name}` yields the invalid Rust identifier `{ident}`; \
                 start the name with a letter and avoid Rust keywords"
            )));
        }
        let type_ident = to_pascal_case(&name);
        let view_ident = format!("{type_ident}View");
        Ok(Self {
            name: name.clone(),
            message_prefix: name.clone(),
            module_ident: ident.clone(),
            type_ident,
            view_ident,
            table_name: format!("{ident}_views"),
            command_broker: format!("{name}-commands"),
            event_broker: format!("{name}-events"),
        })
    }
}

/// A Knative `Trigger` (one per command/event handler).
#[derive(Clone, Debug)]
pub(crate) struct KnativeTrigger {
    pub(crate) name: String,
    pub(crate) broker: String,
    pub(crate) event_type: String,
}

impl KnativeTrigger {
    pub(crate) fn new(event_type: &str, broker: &str, suffix: &str) -> Self {
        Self {
            name: k8s_name(&format!("{}-{suffix}", event_type.replace('.', "-"))),
            broker: broker.to_string(),
            event_type: event_type.to_string(),
        }
    }
}

/// Broker name for a command message (`<owner>-commands`).
pub(crate) fn command_broker_for_message(message_name: &str) -> String {
    format!("{}-commands", message_owner(message_name))
}

/// Broker name for an event message (`<owner>-events`), where `owner` is the
/// first segment of a 3+-segment dotted name, else the first segment.
pub(crate) fn event_broker_for_message(message_name: &str) -> String {
    let parts = message_name
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let owner = if parts.len() >= 3 {
        parts[0]
    } else {
        parts.first().copied().unwrap_or("events")
    };
    format!("{}-events", k8s_name(owner))
}

pub(crate) fn model_scaffolds(raw_models: &[String]) -> Result<Vec<ModelScaffold>, ScaffoldError> {
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for raw_model in raw_models {
        let model = ModelScaffold::new(raw_model)?;
        if !seen.insert(model.name.clone()) {
            return Err(ScaffoldError::new(format!(
                "duplicate model `{}`",
                model.name
            )));
        }
        models.push(model);
    }
    Ok(models)
}

pub(crate) fn default_command_name(names: &ScaffoldNames, models: &[ModelScaffold]) -> String {
    models
        .first()
        .map(|model| format!("{}.create", model.name))
        .unwrap_or_else(|| names.command_name.clone())
}

/// A scaffolded command/event handler and its module identifier.
#[derive(Clone, Debug)]
pub(crate) struct MessageHandler {
    pub(crate) message_name: String,
    pub(crate) module_ident: String,
}

pub(crate) fn message_handlers_with_modules(
    names: Vec<String>,
    fallback_prefix: &str,
    seen_modules: &mut BTreeSet<String>,
) -> Result<Vec<MessageHandler>, ScaffoldError> {
    let mut seen_names = BTreeSet::new();
    let mut handlers = Vec::new();
    for raw_name in names {
        let message_name = raw_name.trim();
        validate_message_name(message_name, fallback_prefix)?;
        if !seen_names.insert(message_name.to_string()) {
            return Err(ScaffoldError::new(format!(
                "duplicate {fallback_prefix} `{message_name}`"
            )));
        }
        let base_module = to_rust_ident(message_name, fallback_prefix);
        let mut module_ident = base_module.clone();
        let mut suffix = 2;
        while !seen_modules.insert(module_ident.clone()) {
            module_ident = format!("{base_module}_{suffix}");
            suffix += 1;
        }
        handlers.push(MessageHandler {
            message_name: message_name.to_string(),
            module_ident,
        });
    }
    Ok(handlers)
}

fn validate_message_name(name: &str, kind: &str) -> Result<(), ScaffoldError> {
    if name.is_empty() {
        return Err(ScaffoldError::new(format!("{kind} name cannot be empty")));
    }
    if name.chars().any(char::is_control) {
        return Err(ScaffoldError::new(format!(
            "{kind} `{name}` contains a control character"
        )));
    }
    Ok(())
}

/// The owning segment of a dotted message name (e.g. `orders` in
/// `orders.create`), normalized to a k8s-safe name. Used to match a command to
/// its model and to derive broker names.
pub(crate) fn message_owner(message_name: &str) -> String {
    message_name
        .split('.')
        .find(|part| !part.is_empty())
        .map(k8s_name)
        .unwrap_or_else(|| "message".to_string())
}

pub(crate) fn k8s_name(value: &str) -> String {
    let name = to_kebab_case(value);
    if name.is_empty() {
        "generated".to_string()
    } else {
        name
    }
}

fn to_rust_ident(value: &str, fallback_prefix: &str) -> String {
    let mut ident = String::new();
    let mut last_was_separator = false;
    for char in value.chars() {
        if char.is_ascii_alphanumeric() {
            ident.push(char.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            ident.push('_');
            last_was_separator = true;
        }
    }
    while ident.ends_with('_') {
        ident.pop();
    }
    while ident.starts_with('_') {
        ident.remove(0);
    }
    if ident.is_empty() {
        ident = fallback_prefix.to_string();
    }
    if ident
        .chars()
        .next()
        .is_some_and(|char| char.is_ascii_digit())
        || is_rust_keyword(&ident)
    {
        ident = format!("{fallback_prefix}_{ident}");
    }
    ident
}

/// True if `value` is a usable Rust identifier given this crate's normalization
/// (already lowercased ASCII alphanumerics and `_`): non-empty, not starting
/// with a digit, and not a reserved keyword. Guards against generated crates
/// whose name would not compile (e.g. a service called `3d`).
fn is_rust_ident(value: &str) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if value
        .chars()
        .any(|char| !(char.is_ascii_alphanumeric() || char == '_'))
    {
        return false;
    }
    !is_rust_keyword(value)
}

fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
    )
}

fn to_kebab_case(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_separator = true;
    for char in input.chars() {
        if char.is_ascii_alphanumeric() {
            out.push(char.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            out.push('-');
            last_was_separator = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn to_pascal_case(input: &str) -> String {
    input
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut out = String::new();
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
            out
        })
        .collect()
}

/// Escape a string as a Rust/TOML string literal.
pub(crate) fn rust_string(value: &str) -> String {
    toml_string(value)
}

/// Escape a string as a TOML string literal.
pub(crate) fn toml_string(value: impl AsRef<str>) -> String {
    serde_json::to_string(value.as_ref()).expect("string serialization should succeed")
}
