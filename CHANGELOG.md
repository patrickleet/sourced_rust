### What's changed in v4.4.0

* feat: allow has_many onto composite-key child models (#210) (by @patrickleet)

  * feat: allow has_many onto composite-key child models

  GraphQL join compilation already uses a single-column FK
  (child.fk = parent.pk). The surface validator still rejected any
  composite-PK model that participated in a relationship, which forced
  apps to stuff child collections into JSON blobs or invent a surrogate
  id.

  Keep rejecting joins that actually need a composite identity
  (belongs_to targeting a composite PK, has_many from a composite parent,
  m2m). A workspace with a single-column PK can now has_many projects
  whose identity is (workspace_id, path).

  * feat: join composite keys through many-to-many tables

  Many-to-many is a join table, not a direct PK equality. Each end's
  full primary key maps to same-named columns on the through table
  (single-column ends still use foreign_key / target_foreign_key).

  Nested queries and relationship filters AND those equalities, so a
  composite project can list labels without inventing a surrogate id.

  * refactor: resolve m2m join keys once in table/

  Pair through columns with each end's PK in table/, not compile/.
  Explicit foreign_key/target_foreign_key lists through columns in PK
  order; otherwise same-named through columns or unique column FKs.
  Compile formats the pair list. Surface through keys are vectors.
  HasMany SQL uses has_many_join_columns instead of .first()/id.

  * feat: join composite keys on direct has_many and belongs_to

  foreign_key lists FK columns in the other end's PK order. Compile ANDs
  those equalities. A partial list is rejected instead of taking .first().

  * fix: keep missing-FK registry error wording

  HasMany still reports "foreign key \`x\` is not a column on target model".

  * test: cover composite filter exclusion and m2m row-policy keys

  Row policies now resolve m2m join keys, including operational targets.
  The has_many filter fixture includes a non-matching workspace.

  * docs: document composite read-model relationship keys

  GraphQL ANDs PK-order foreign_key lists for has_many, belongs_to, and
  many_to_many. ORM includes stay one-level and single-column.


See full diff: [v4.3.0...v4.4.0](https://github.com/hops-ops/distributed/compare/v4.3.0...v4.4.0)
