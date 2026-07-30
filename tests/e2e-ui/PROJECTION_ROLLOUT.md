# Projection rollout and rollback

This runbook covers the e2e-ui migration to catalog-pinned modeled
projections. Identifiers are examples from the fixture; production deployment
automation must use the exact generated program IDs, binding IDs, schemas,
epochs, and routes from its catalog.

## Artifact ownership

| Artifact | Owner | Required evidence |
|---|---|---|
| `TodoState` and Todo aggregate events | Todo bounded context | Domain-state capture and replay tests |
| `ChatMessageState` and Chat aggregate events | Chat bounded context | Domain-state capture and replay tests |
| `BlobGameState` and Blob aggregate events | Blob bounded context | Domain-state capture and replay tests |
| `Todos`, `ChatMessages`, `BlobGames`, and their projection programs | Read-model catalog owner | Program vectors, output schema pins, and direct-eligibility tests |
| `AuthUsers` and provider mapping | Identity integration adapter | Ingestor mapping and authorization tests |
| Forward query relationships to `AuthUsers` | Deployment read-model catalog owner | Schema/storage-identity regression tests |
| Projection source, owner, physical topology, route, and activation | Service deployment owner | Catalog validation and exact active-binding handshake |
| Generated manifest/TypeScript/SDL | Client compiler owner | Generate then byte-for-byte check mode |
| Projector checkpoints/failure rows | Projector runtime owner | Drain and replay observations |
| Rebuild/import completion marker | Data migration owner | Durable marker tied to binding/epoch and source high-water mark |
| Obligation-minting disable switch | Command edge operator | Audited state change plus drain observation |

Name one accountable person or on-call rotation for every owner before a
production cutover.

## Compatibility probe

Before activation, every producer, consumer, command edge, and client compiler
must accept the candidate catalog:

1. Exchange the canonical catalog bytes and digest out of band.
2. Verify the program ID, binding ID, event selectors/body fingerprints,
   partition codec/version, output storage schemas, relationship pins, physical
   topology, and epoch.
3. Run the consumer in probe-only mode against representative envelopes,
   including deletion, partial-preview recovery, and unknown-field fallback.
4. Reject unknown wire/IR/operation-semantics versions, schema drift, duplicate
   writers, direct/eventual overlap, or an old binary claiming the new epoch.
5. Record the exact catalog digest and compatible binary versions as the
   signed compatibility result. A health check or reachable endpoint is not a
   compatibility result.

## Remote-binding activation handshake

A remote binding is producer-visible before it is locally executable. Activate
it only after the remote consumer advertises:

- the exact binding ID and program ID;
- the candidate epoch and `Active` lifecycle state;
- its logical executor route (never an endpoint or credential);
- the physical topology identity and partition codec;
- readiness from the source high-water mark or an explicit rebuild/import
  marker.

The deployment owner then atomically publishes the catalog activation.
Producers may mint obligations only for an `Active` causal binding whose exact
handshake is present. `Draining`, background, missing-route, or mismatched
bindings never receive new obligations.

## Cutover sequence

1. Freeze the old catalog digest and record the last safe rollback checkpoint:
   source high-water mark, old projector checkpoint, read-model backup/import
   marker, and outstanding obligation count.
2. Deploy compatible readers/consumers in probe-only or draining-safe mode.
3. Rebuild or import query rows when the topology identity changed. Store a
   durable completion marker containing source position, program ID, binding
   ID, epoch, row counts/checksum, and operator.
4. Complete the remote activation handshake.
5. Activate the new binding. Never overlap direct and eventual writers for the
   same output scope.
6. Enable obligation minting for the new active binding.
7. Observe publication, ordered checkpoints, command terminal states,
   projection failures, and replica convergence.
8. Mark the old binding `Draining`.
9. Remove the old consumer only after its durable inbox is empty, every old
   obligation is terminal, its checkpoint reached the recorded cutover source
   position, and no producer can mint its binding identity.

## Stop-minting and drain

The first incident control is the obligation-minting disable switch for the
candidate binding. It must stop new command obligations without discarding
already committed domain events or ledger records.

After disabling:

1. Keep the compatible consumer running.
2. Drain already committed occurrences and obligations.
3. Preserve failure rows and retry evidence.
4. Do not let an old binary take over the new epoch.
5. Decide whether to resume the exact binding, rebuild into a newer epoch, or
   roll back from the recorded checkpoint.

This switch is not data rollback and must not silently rewrite causal outcomes
as succeeded.

## Rollback

Rollback is safe without a rebuild only before the new binding has written rows
or minted durable obligations. That recorded moment is the last safe rollback
checkpoint.

After writes or obligations exist:

- stop minting new obligations;
- drain the compatible new consumer;
- restore/rebuild/import read models from the recorded source position;
- persist the rebuild/import completion marker;
- re-run the old consumer compatibility probe against the catalog it will own;
- prove there is no active or draining writer overlap;
- activate a fresh epoch rather than inventing revisions for pre-protocol rows.

Never delete committed history, forge projector observations, reuse an epoch
with a different topology, or expose database URLs/tokens in rollout
diagnostics.

## Fixture identities

The fixture uses these logical owners and epochs:

| Mode | Owner | Epoch |
|---|---|---|
| Todo causal/eventual | `project_todos` | `e2e-ui-todos-v2` |
| Chat causal/eventual | `project_chat_messages` | `e2e-ui-chat-v2` |
| Blob direct/projected | `project_blob` | `e2e-ui-blob-v2` |

Program and binding IDs are generated digests; never copy or infer them from
these names.
