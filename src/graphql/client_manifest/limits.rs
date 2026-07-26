use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientExecutionLimits {
    pub max_depth: u64,
    pub max_complexity: u64,
    pub max_bool_width: u64,
    pub max_in_list: u64,
    pub complexity: ClientComplexityWeights,
}

impl Default for ClientExecutionLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH as u64,
            max_complexity: DEFAULT_MAX_COMPLEXITY as u64,
            max_bool_width: DEFAULT_MAX_BOOL_WIDTH,
            max_in_list: DEFAULT_MAX_IN_LIST,
            complexity: ClientComplexityWeights::current(),
        }
    }
}

impl ClientExecutionLimits {
    pub(crate) fn from_runtime(
        max_depth: usize,
        max_complexity: usize,
        max_bool_width: usize,
        max_in_list: usize,
    ) -> Result<Self, ClientManifestError> {
        Ok(Self {
            max_depth: u64::try_from(max_depth).map_err(|_| {
                ClientManifestError("GraphQL max_depth exceeds the client manifest range".into())
            })?,
            max_complexity: u64::try_from(max_complexity).map_err(|_| {
                ClientManifestError(
                    "GraphQL max_complexity exceeds the client manifest range".into(),
                )
            })?,
            max_bool_width: u64::try_from(max_bool_width).map_err(|_| {
                ClientManifestError(
                    "GraphQL max_bool_width exceeds the client manifest range".into(),
                )
            })?,
            max_in_list: u64::try_from(max_in_list).map_err(|_| {
                ClientManifestError("GraphQL max_in_list exceeds the client manifest range".into())
            })?,
            complexity: ClientComplexityWeights::current(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientComplexityWeights {
    pub version: u32,
    pub scalar: u64,
    pub belongs_to: u64,
    pub has_many: u64,
    pub m2m: u64,
    pub aggregate: u64,
    pub list_root: u64,
    pub by_pk: u64,
    pub list_fanout: u64,
}

impl ClientComplexityWeights {
    pub(super) fn current() -> Self {
        let weights = default_weights();
        Self {
            version: QUERY_COMPLEXITY_VERSION,
            scalar: weights.scalar as u64,
            belongs_to: weights.belongs_to as u64,
            has_many: weights.has_many as u64,
            m2m: weights.m2m as u64,
            aggregate: weights.aggregate as u64,
            list_root: weights.list_root as u64,
            by_pk: weights.by_pk as u64,
            list_fanout: weights.list_fanout as u64,
        }
    }
}
