distributed::module! {
    pub UNLISTED_MODULE {
        id: "unlisted-module",
        commands: [],
        projections: [],
        surfaces: [],
    }
}

pub fn marker() -> &'static str {
    UNLISTED_MODULE.manifest().id.as_str()
}
