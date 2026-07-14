/**
 * Generate command client artifacts from commands.manifest.json.
 *
 * Outputs (single source of mutation text):
 *   1. commands.operations.gql  — GraphiQL / human copy-paste
 *   2. commands.generated.ts    — same documents + typed functions
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

  for (const cmd of catalog.commands) {
    const fn = fieldToFnName(cmd.field_name);
    const inType = cmd.input?.name ?? 'Record<string, unknown>';
    const outType = cmd.output?.name ?? 'Record<string, unknown>';
    const hasInput = !!cmd.input;

    lines.push(`/**`);
    lines.push(` * ${cmd.command_name} → GraphQL \`${cmd.field_name}\``);
    lines.push(` * roles: ${(cmd.roles || []).join(', ') || '(any)'}`);
    lines.push(` * @param client Bound GraphQL client (\`useGraphql(() => data)\`)`);
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

  const catalog = JSON.parse(fs.readFileSync(inPath, 'utf8'));
  const ts = generateCommandsTs(catalog);
  const gql = generateOperationsGql(catalog);

  fs.mkdirSync(path.dirname(outTs), { recursive: true });
  fs.writeFileSync(outTs, ts.endsWith('\n') ? ts : ts + '\n');
  fs.writeFileSync(outGql, gql.endsWith('\n') ? gql : gql + '\n');
  console.error(
    `gen-commands: wrote ${path.relative(uiRoot, outTs)} + ${path.relative(uiRoot, outGql)} (${catalog.commands.length} commands)`
  );
}

const isMain =
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main();
}
