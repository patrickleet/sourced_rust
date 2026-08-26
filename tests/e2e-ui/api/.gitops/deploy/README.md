# e2e-ui API cloud chart

This chart renders the packaged API image for cloud Environments. It is
independent from `.gitops/local` and rejects `local: true` so local development
cannot accidentally select the cloud workload posture.
