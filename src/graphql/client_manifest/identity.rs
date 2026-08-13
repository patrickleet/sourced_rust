use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientSurfaceIdentity {
    Role { name: String },
    Application { name: String, roles: Vec<String> },
}

impl ClientSurfaceIdentity {
    pub fn role(name: impl Into<String>) -> Self {
        Self::Role { name: name.into() }
    }

    pub fn application(
        name: impl Into<String>,
        roles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut roles: Vec<String> = roles.into_iter().map(Into::into).collect();
        roles.sort();
        roles.dedup();
        Self::Application {
            name: name.into(),
            roles,
        }
    }

    pub(super) fn canonicalized(self) -> Result<Self, ClientManifestError> {
        match self {
            Self::Role { name } if name.trim().is_empty() => Err(ClientManifestError(
                "role surface name must not be empty".into(),
            )),
            Self::Role { name } => Ok(Self::Role { name }),
            Self::Application { name, .. } if name.trim().is_empty() => Err(ClientManifestError(
                "application surface name must not be empty".into(),
            )),
            Self::Application { name, mut roles } => {
                if roles.iter().any(|role| role.trim().is_empty()) {
                    return Err(ClientManifestError(format!(
                        "application surface `{name}` contains an empty role id"
                    )));
                }
                roles.sort();
                roles.dedup();
                if roles.is_empty() {
                    return Err(ClientManifestError(format!(
                        "application surface `{name}` must declare at least one role"
                    )));
                }
                Ok(Self::Application { name, roles })
            }
        }
    }
}
