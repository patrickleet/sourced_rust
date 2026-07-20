/**
 * Generate command client artifacts from commands.manifest.json.
 *
 * Outputs (single source of mutation text + pipeline policies):
 *   1. commands.operations.gql           — GraphiQL / human copy-paste
 *   2. commands.generated.ts             — same documents + typed functions
 *   3. commands.policies.generated.ts    — client_reconcile → CommandPolicy map
 *
 * Functions take the bound GraphQL client (`useGraphql(() => data)`), which
 * already owns URL + auth headers — no { url, auth } boilerplate.
 *
 * Usage (from tests/e2e-ui/ui):
 *   node scripts/gen-commands.mjs
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const uiRoot = path.resolve(here, '..');

/**
 * Build multiline mutation operation for one catalog command.
 * @param {object} cmd
 * @returns {{ name: string, lines: string[], text: string }}
 */
export function buildMutationOp(cmd) {
  const hasInput = !!cmd.input;
  const selLines = selectionSetLines(cmd.output);
  const inputTypeGql = cmd.input ? `${cmd.input.name}!` : null;
  const opName = `Command_${cmd.field_name}`;

  const fieldLine = hasInput
    ? `  ${cmd.field_name}(input: $input)`
    : `  ${cmd.field_name}`;
  const fieldWithSel =
    selLines.length === 0
      ? [fieldLine]
      : [
          `${fieldLine} {`,
          ...selLines.slice(1, -1).map((l) => `  ${l}`),
          `  }`,
        ];

  const lines = hasInput
    ? [
        `mutation ${opName}($input: ${inputTypeGql}) {`,
        ...fieldWithSel,
        `}`,
      ]
    : [`mutation ${opName} {`, ...fieldWithSel, `}`];

  return { name: opName, lines, text: lines.join('\n') };
}

/**
 * @param {object} catalog
 * @returns {string}
 */
export function generateOperationsGql(catalog) {
  assertCatalog(catalog);
  const parts = [
    '# GENERATED — do not edit by hand.',
    '# Source: e2e_service::graphql_commands() → commands.manifest.json',
    '# Regenerate: `make gen-commands` (from tests/e2e-ui)',
    '# Copy into GraphiQL; app code uses commands.generated.ts (same text).',
    '# Spec: distributed GitKB specs/query-layer/references/command-client-dx',
    '',
  ];
  for (const cmd of catalog.commands) {
    const { text } = buildMutationOp(cmd);
    parts.push(`# ${cmd.command_name} — roles: ${(cmd.roles || []).join(', ') || '(any)'}`);
    parts.push(text);
    parts.push('');
  }
  return parts.join('\n');
}

/**
 * @param {object} catalog
 * @returns {string}
 */
export function generateCommandsTs(catalog) {
  assertCatalog(catalog);

  const lines = [];
  lines.push('/**');
  lines.push(' * GENERATED — do not edit by hand.');
  lines.push(' * Source: e2e_service::graphql_commands() → commands.manifest.json');
  lines.push(' * Documents mirror `commands.operations.gql` (copy-paste for GraphiQL).');
  lines.push(' * Regenerate: `make gen-commands` (from tests/e2e-ui)');
  lines.push(' * Spec: distributed GitKB specs/query-layer/references/command-client-dx');
  lines.push(' */');
  lines.push(`import type { GqlDocument } from '../gql/document.ts';`);
  lines.push(`import type { GqlResult } from '../gql/types.ts';`);
  lines.push('');
  lines.push('/** Bound client from `useGraphql(() => data)` / `createGraphqlClient`. */');
  lines.push('export type CommandClient = {');
  lines.push('  request: <');
  lines.push('    TResult = Record<string, unknown>,');
  lines.push('    TVariables extends Record<string, unknown> = Record<string, unknown>');
  lines.push('  >(');
  lines.push('    document: GqlDocument,');
  lines.push('    variables?: TVariables');
  lines.push('  ) => Promise<GqlResult<TResult>>;');
  lines.push('};');
  lines.push('');

  /** @type {Map<string, object>} */
  const typeDefs = new Map();
  for (const cmd of catalog.commands) {
    if (cmd.input) typeDefs.set(cmd.input.name, cmd.input);
    if (cmd.output) typeDefs.set(cmd.output.name, cmd.output);
  }

  for (const t of typeDefs.values()) {
    lines.push(`export type ${t.name} = {`);
    for (const f of t.fields) {
      lines.push(`  ${f.name}${f.nullable ? '?' : ''}: ${gqlScalarToTs(f)};`);
    }
    lines.push('};');
    lines.push('');
  }

  lines.push('/** Field name → roles that may execute (engine ACL; client is not a boundary). */');
  lines.push('export const COMMAND_ROLES = {');
  for (const cmd of catalog.commands) {
    const roles = (cmd.roles || []).map((r) => JSON.stringify(r)).join(', ');
    lines.push(`  ${JSON.stringify(cmd.field_name)}: [${roles}] as const,`);
  }
  lines.push('} as const;');
  lines.push('');

  // Shared document constants (same text as commands.operations.gql).
  lines.push('/** GraphQL mutation documents — keep in sync with commands.operations.gql. */');
  lines.push('export const COMMAND_DOCS = {');
  for (const cmd of catalog.commands) {
    const { text } = buildMutationOp(cmd);
    lines.push(`  ${JSON.stringify(cmd.field_name)}: \``);
    for (const ml of text.split('\n')) {
      lines.push(escapeTemplateLine(ml));
    }
    lines.push('`,');
  }
  lines.push('} as const;');
  lines.push('');

  /** @type {Array<{ fn: string, hasInput: boolean, inType: string, outType: string }>} */
  const fnMeta = [];

  for (const cmd of catalog.commands) {
    const fn = fieldToFnName(cmd.field_name);
    const inType = cmd.input?.name ?? 'Record<string, unknown>';
    const outType = cmd.output?.name ?? 'Record<string, unknown>';
    const hasInput = !!cmd.input;
    fnMeta.push({ fn, hasInput, inType, outType });

    lines.push(`/**`);
    lines.push(` * ${cmd.command_name} → GraphQL \`${cmd.field_name}\``);
    lines.push(` * roles: ${(cmd.roles || []).join(', ') || '(any)'}`);
    lines.push(` * Prefer \`client.commands.${fn}(…)\` via \`bindCommands\` / \`useGraphql\`.`);
    lines.push(` */`);
    if (hasInput) {
      lines.push(
        `export async function ${fn}(input: ${inType}, client: CommandClient): Promise<GqlResult<${outType}>> {`
      );
      lines.push(
        `  const result = await client.request<{ ${cmd.field_name}?: ${outType} }>(COMMAND_DOCS[${JSON.stringify(cmd.field_name)}], { input });`
      );
    } else {
      lines.push(
        `export async function ${fn}(client: CommandClient): Promise<GqlResult<${outType}>> {`
      );
      lines.push(
        `  const result = await client.request<{ ${cmd.field_name}?: ${outType} }>(COMMAND_DOCS[${JSON.stringify(cmd.field_name)}]);`
      );
    }
    lines.push(`  return {`);
    lines.push(`    data: result.data?.${cmd.field_name},`);
    lines.push(`    errors: result.errors,`);
    lines.push(`    status: result.status`);
    lines.push(`  };`);
    lines.push(`}`);
    lines.push('');
  }

  // Bound surface: client.commands.todosCreate(input)
  lines.push('/** Commands pre-bound to a GraphQL client (URL + auth already configured). */');
  lines.push('export type BoundCommands = {');
  for (const m of fnMeta) {
    if (m.hasInput) {
      lines.push(
        `  ${m.fn}: (input: ${m.inType}) => Promise<GqlResult<${m.outType}>>;`
      );
    } else {
      lines.push(`  ${m.fn}: () => Promise<GqlResult<${m.outType}>>;`);
    }
  }
  lines.push('};');
  lines.push('');
  lines.push('/**');
  lines.push(' * Register all command helpers on a client once:');
  lines.push(' * `const gql = useGraphql(() => data); await gql.commands.todosCreate(input)`');
  lines.push(' */');
  lines.push('export function bindCommands(client: CommandClient): BoundCommands {');
  lines.push('  return {');
  for (const m of fnMeta) {
    if (m.hasInput) {
      lines.push(`    ${m.fn}: (input) => ${m.fn}(input, client),`);
    } else {
      lines.push(`    ${m.fn}: () => ${m.fn}(client),`);
    }
  }
  lines.push('  };');
  lines.push('}');
  lines.push('');

  return lines.join('\n');
}

/**
 * Generate TS command pipeline policies from catalog `client_reconcile` hints.
 * Keys are bound function names (`todosCreate`), matching `CommandPolicyMap`.
 *
 * @param {object} catalog
 * @returns {string}
 */
export function generateCommandPoliciesTs(catalog) {
  assertCatalog(catalog);

  const lines = [];
  lines.push('/**');
  lines.push(' * GENERATED — do not edit by hand.');
  lines.push(' * Source: e2e_service::graphql_commands() → client_reconcile → commands.manifest.json');
  lines.push(' * Regenerate: `make gen-commands` (from tests/e2e-ui)');
  lines.push(' * Spec: distributed GitKB tasks/graphql-qs-command-return-4 (D3)');
  lines.push(' */');
  lines.push(`import type { CommandPolicyMap } from '../gql/bind-commands-pipeline.ts';`);
  lines.push('');
  lines.push('/**');
  lines.push(' * Default result/reconcile policies from the Rust command registry.');
  lines.push(' * Call-site options on `gql.commands.*(input, opts)` still win.');
  lines.push(' */');
  lines.push('export const e2eCommandPolicies: CommandPolicyMap = {');

  for (const cmd of catalog.commands) {
    const cr = cmd.client_reconcile;
    if (!cr?.result?.kind) continue;
    const fn = fieldToFnName(cmd.field_name);
    const resultKind = String(cr.result.kind);
    const reconcileKind = cr.reconcile?.kind ? String(cr.reconcile.kind) : 'none';
    lines.push(`\t${fn}: {`);
    lines.push(`\t\tresult: { kind: ${JSON.stringify(resultKind)} },`);
    lines.push(`\t\treconcile: { kind: ${JSON.stringify(reconcileKind)} }`);
    lines.push(`\t},`);
  }

  lines.push('};');
  lines.push('');
  return lines.join('\n');
}

function assertCatalog(catalog) {
  if (!catalog || catalog.version !== 1 || !Array.isArray(catalog.commands)) {
    throw new Error('invalid command catalog: expected { version: 1, commands: [...] }');
  }
}

function escapeTemplateLine(ml) {
  return ml.replace(/\\/g, '\\\\').replace(/`/g, '\\`').replace(/\$\{/g, '\\${');
}

/**
 * @param {{ type_name: string, list?: boolean, nullable?: boolean }} f
 */
function gqlScalarToTs(f) {
  let base;
  switch (f.type_name) {
    case 'String':
    case 'ID':
      base = 'string';
      break;
    case 'Boolean':
      base = 'boolean';
      break;
    case 'Int':
    case 'Float':
    case 'BigInt':
      base = 'number';
      break;
    case 'JSON':
      base = 'unknown';
      break;
    default:
      base = f.type_name;
  }
  if (f.list) base = `${base}[]`;
  return base;
}

/** todos_create → todosCreate */
export function fieldToFnName(field) {
  return String(field).replace(/_([a-z])/g, (_, c) => c.toUpperCase());
}

/**
 * @param {object | null | undefined} output
 * @returns {string[]}
 */
function selectionSetLines(output) {
  if (!output?.fields?.length) return [];
  const inner = output.fields.map((f) => `  ${f.name}`);
  return ['{', ...inner, '}'];
}

function main() {
  const inPath =
    process.argv[2] || path.join(uiRoot, 'src/lib/api/commands.manifest.json');
  const outTs =
    process.argv[3] || path.join(uiRoot, 'src/lib/api/commands.generated.ts');
  const outGql =
    process.argv[4] || path.join(uiRoot, 'src/lib/api/commands.operations.gql');
  const outPolicies =
    process.argv[5] || path.join(uiRoot, 'src/lib/api/commands.policies.generated.ts');

  const catalog = JSON.parse(fs.readFileSync(inPath, 'utf8'));
  const ts = generateCommandsTs(catalog);
  const gql = generateOperationsGql(catalog);
  const policies = generateCommandPoliciesTs(catalog);

  fs.mkdirSync(path.dirname(outTs), { recursive: true });
  fs.writeFileSync(outTs, ts.endsWith('\n') ? ts : ts + '\n');
  fs.writeFileSync(outGql, gql.endsWith('\n') ? gql : gql + '\n');
  fs.writeFileSync(outPolicies, policies.endsWith('\n') ? policies : policies + '\n');
  const withPolicy = catalog.commands.filter((c) => c.client_reconcile).length;
  console.error(
    `gen-commands: wrote ${path.relative(uiRoot, outTs)} + ${path.relative(uiRoot, outGql)} + ${path.relative(uiRoot, outPolicies)} (${catalog.commands.length} commands, ${withPolicy} policies)`
  );
}

const isMain =
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main();
}
