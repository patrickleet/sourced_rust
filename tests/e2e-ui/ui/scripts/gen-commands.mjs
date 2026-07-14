/**
 * Generate TypeScript command client from commands.manifest.json.
 *
 * Pure transform: registry → async functions that POST GraphQL mutations via
 * requestGraphql. Callers supply url + auth (no secrets in the generated module).
 *
 * Usage (from tests/e2e-ui/ui):
 *   node scripts/gen-commands.mjs
 *   node scripts/gen-commands.mjs path/to/commands.manifest.json path/to/out.ts
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const uiRoot = path.resolve(here, '..');

/**
 * @param {object} catalog
 * @returns {string}
 */
export function generateCommandsTs(catalog) {
  if (!catalog || catalog.version !== 1 || !Array.isArray(catalog.commands)) {
    throw new Error('invalid command catalog: expected { version: 1, commands: [...] }');
  }

  const lines = [];
  lines.push('/**');
  lines.push(' * GENERATED — do not edit by hand.');
  lines.push(' * Source: e2e_service::graphql_commands() → commands.manifest.json');
  lines.push(' * Regenerate: `make gen-commands` (from tests/e2e-ui)');
  lines.push(' * Spec: distributed GitKB specs/query-layer/references/command-client-dx');
  lines.push(' */');
  lines.push(`import { requestGraphql } from '../gql/request.ts';`);
  lines.push(`import type { GqlAuth, GqlResult } from '../gql/types.ts';`);
  lines.push('');
  lines.push('export type CommandRequestOpts = {');
  lines.push('  /** Absolute or same-origin GraphQL URL, e.g. `/graphql` */');
  lines.push('  url: string;');
  lines.push('  auth?: GqlAuth;');
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

  for (const cmd of catalog.commands) {
    const fn = fieldToFnName(cmd.field_name);
    const inType = cmd.input?.name ?? 'Record<string, unknown>';
    const outType = cmd.output?.name ?? 'Record<string, unknown>';
    const hasInput = !!cmd.input;
    const selLines = selectionSetLines(cmd.output);
    const inputTypeGql = cmd.input ? `${cmd.input.name}!` : null;

    // Multiline GraphQL for human-readable generated source (template literal).
    // Selection set is nested under the field for normal GraphQL style.
    const fieldLine = hasInput
      ? `  ${cmd.field_name}(input: $input)`
      : `  ${cmd.field_name}`;
    const fieldWithSel =
      selLines.length === 0
        ? [fieldLine]
        : [
            `${fieldLine} {`,
            ...selLines.slice(1, -1).map((l) => `  ${l}`), // indent field names one more level
            `  }`,
          ];
    const mutLines = hasInput
      ? [
          `mutation Command_${cmd.field_name}($input: ${inputTypeGql}) {`,
          ...fieldWithSel,
          `}`,
        ]
      : [`mutation Command_${cmd.field_name} {`, ...fieldWithSel, `}`];

    lines.push(`/**`);
    lines.push(` * ${cmd.command_name} → GraphQL \`${cmd.field_name}\``);
    lines.push(` * roles: ${(cmd.roles || []).join(', ') || '(any)'}`);
    lines.push(` */`);
    if (hasInput) {
      lines.push(
        `export async function ${fn}(input: ${inType}, opts: CommandRequestOpts): Promise<GqlResult<${outType}>> {`
      );
    } else {
      lines.push(
        `export async function ${fn}(opts: CommandRequestOpts): Promise<GqlResult<${outType}>> {`
      );
    }
    lines.push(`  const document = \``);
    for (const ml of mutLines) {
      // Escape backticks/$ in generated GraphQL (none expected in field names).
      lines.push(ml.replace(/\\/g, '\\\\').replace(/`/g, '\\`').replace(/\$\{/g, '\\${'));
    }
    lines.push(`\`;`);
    if (hasInput) {
      lines.push(
        `  const result = await requestGraphql<{ ${cmd.field_name}?: ${outType} }>(opts.url, document, opts.auth ?? {}, { input });`
      );
    } else {
      lines.push(
        `  const result = await requestGraphql<{ ${cmd.field_name}?: ${outType} }>(opts.url, document, opts.auth ?? {}, {});`
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
      base = f.type_name; // nested object type name if ever expanded
  }
  if (f.list) base = `${base}[]`;
  return base;
}

/** todos_create → todosCreate */
export function fieldToFnName(field) {
  return String(field).replace(/_([a-z])/g, (_, c) => c.toUpperCase());
}

/**
 * Multiline selection set lines (no trailing commas) for readable GraphQL.
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
  const outPath =
    process.argv[3] || path.join(uiRoot, 'src/lib/api/commands.generated.ts');
  const raw = fs.readFileSync(inPath, 'utf8');
  const catalog = JSON.parse(raw);
  const ts = generateCommandsTs(catalog);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, ts.endsWith('\n') ? ts : ts + '\n');
  console.error(
    `gen-commands: wrote ${outPath} (${catalog.commands.length} commands from ${inPath})`
  );
}

const isMain =
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main();
}
