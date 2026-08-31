# e2e-ui API cloud chart

This chart renders the packaged API image for cloud Environments. It is
independent from `.gitops/local` and rejects `local: true` so local development
cannot accidentally select the cloud workload posture. Set
`image.tag` to `vMAJOR.MINOR.PATCH` for a release or
`pr-NUMBER-40_HEX_SHA` for a preview before applying it. The chart rejects
empty and arbitrary mutable tags.
