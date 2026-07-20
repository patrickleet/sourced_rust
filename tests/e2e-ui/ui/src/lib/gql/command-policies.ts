/**
 * Command pipeline policies for e2e-ui.
 *
 * **Source of truth:** Rust `exposed_command().client_reconcile(...)` in
 * `e2e_service::graphql_commands()`, exported via `commands.manifest.json`,
 * generated into `commands.policies.generated.ts` by `make gen-commands`.
 *
 * Do not hand-edit the generated file — change the Rust registry instead.
 */
export { e2eCommandPolicies } from '../api/commands.policies.generated.ts';
