### What's changed in v3.2.0

* feat(cli): dctl skills init — extract embedded agent skills into .distributed/ (#113) (by @patrickleet)

  * feat(cli): add dctl skills init/list — embedded agent skills

  Materializes agent skills (distributed-usage, distributed-ci,
  distributed-schema) embedded via include_str! into .distributed/skills/,
  with harness wiring adapters: .claude/skills/ copies for Claude Code and
  .agents/skills/ copies + a sentinel-managed AGENTS.md block for Codex,
  Grok, Gemini, Pi, and AGENTS.md-only tools. Pure generation returns
  GeneratedProject; per-file drift semantics (created/unchanged/skipped/
  updated) make re-runs idempotent and never clobber local edits without
  --force.

  Implements [[tasks/cli-skills-init]]

  * fix: clarify skills init upgrade behavior

  Implements [[feat/cli-skills-init]]

  * refactor(cli): wire harness skill locations as symlinks to .distributed

  One on-disk copy: canonical skills stay under <container>/skills/; each
  wired harness location (.claude/skills/<name>, .agents/skills/<name>)
  becomes a relative per-skill symlink to the canonical folder, coexisting
  with user-owned skills. Non-unix platforms fall back to real copies.
  A non-link path at a harness location (stale link or old copy layout)
  is skipped with a warning and converted with --force.

  Implements [[tasks/cli-skills-init]]

  * docs(skills): cover --metrics prometheus and --tracing in distributed-ci

  Now that the observability generators are on main (#100), the CI skill
  documents ServiceMonitor/PrometheusRule gating and the OTLP env values.

  Implements [[tasks/cli-skills-init]]

  * docs(skills): lead distributed-usage with the models-and-handlers thesis

  The main point of using Distributed: the authored surface is models and
  handlers; the framework and dctl generate the deterministic structure
  around them. Emphasized in the skill body and its trigger description.

  Implements [[tasks/cli-skills-init]]

  * docs(skills): prefer the highest-level macro APIs in distributed-usage

  #[sourced] over #[digest]+aggregate!(), the derives over hand plumbing,
  routes! and with_bus(..).run(..) over manual wiring — dropping a level is
  a deliberate choice, not a default.

  Implements [[tasks/cli-skills-init]]


See full diff: [v3.1.0...v3.2.0](https://github.com/hops-ops/distributed/compare/v3.1.0...v3.2.0)
