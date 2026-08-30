# e2e-ui UI cloud chart

This chart renders the packaged UI image for cloud Environments. It is
independent from `.gitops/local` and rejects `local: true` so local development
cannot accidentally select the cloud workload posture. Set
`image.tag` to an immutable release/build tag before applying it; the chart
rejects an empty tag and `latest`.
