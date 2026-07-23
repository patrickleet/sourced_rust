#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { generateCommandArtifacts, parseCodegenManifest } from '../codegen.js';

type CliPaths = {
  cwd: string;
  manifest: string;
  commands: string;
  operations: string;
  policies: string;
};

const DEFAULT_PATHS = {
  manifest: 'src/lib/api/commands.manifest.json',
  commands: 'src/lib/api/commands.generated.ts',
  operations: 'src/lib/api/commands.operations.gql',
  policies: 'src/lib/api/commands.policies.generated.ts'
} as const;

const HELP = `Usage: distributed-gen-commands [options]

Generate app-owned TypeScript commands, GraphQL operations, and client policies
from a Distributed command manifest (legacy version 1 or client manifest v4).

Options:
  --cwd <directory>       Base directory for relative paths (default: cwd)
  --manifest <path>       Input manifest (default: ${DEFAULT_PATHS.manifest})
  --commands <path>       Generated TypeScript (default: ${DEFAULT_PATHS.commands})
  --operations <path>     Generated GraphQL (default: ${DEFAULT_PATHS.operations})
  --policies <path>       Generated policies (default: ${DEFAULT_PATHS.policies})
  -h, --help              Show this help
`;

export function runCommandCodegenCli(
  argv: string[] = process.argv.slice(2),
  invocationCwd: string = process.cwd()
): void {
  const parsed = parseArguments(argv);
  if (parsed.help) {
    process.stdout.write(HELP);
    return;
  }

  const paths = resolvePaths(parsed.values, invocationCwd);
  assertDistinctPaths(paths);

  let source: string;
  try {
    source = fs.readFileSync(paths.manifest, 'utf8');
  } catch (error) {
    throw new Error(`failed to read manifest ${paths.manifest}: ${errorMessage(error)}`);
  }

  let json: unknown;
  try {
    json = JSON.parse(source);
  } catch (error) {
    throw new Error(`invalid JSON in ${paths.manifest}: ${errorMessage(error)}`);
  }

  const manifest = parseCodegenManifest(json);
  const artifacts = generateCommandArtifacts(manifest, {
    commandsImport: relativeNodeImport(paths.policies, paths.commands)
  });

  // Validate and generate everything before making any output mutation.
  writeGeneratedFile(paths.commands, artifacts.commands);
  writeGeneratedFile(paths.operations, artifacts.operations);
  writeGeneratedFile(paths.policies, artifacts.policies);

  const policyCount =
    'version' in manifest
      ? manifest.commands.filter(
          (command) =>
            command.client_reconcile?.result || command.client_reconcile?.reconcile
        ).length
      : 0;
  process.stderr.write(
    `distributed-gen-commands: wrote ${displayPath(paths.cwd, paths.commands)}, ` +
      `${displayPath(paths.cwd, paths.operations)}, and ` +
      `${displayPath(paths.cwd, paths.policies)} ` +
      `(${manifest.commands.length} commands, ${policyCount} policies)\n`
  );
}

type ParsedArguments = {
  help: boolean;
  values: Partial<Record<keyof typeof DEFAULT_PATHS | 'cwd', string>>;
};

function parseArguments(argv: string[]): ParsedArguments {
  const values: ParsedArguments['values'] = {};
  let help = false;

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]!;
    if (argument === '--help' || argument === '-h') {
      help = true;
      continue;
    }

    const equal = argument.indexOf('=');
    const flag = equal >= 0 ? argument.slice(0, equal) : argument;
    const inlineValue = equal >= 0 ? argument.slice(equal + 1) : undefined;
    if (!isPathFlag(flag)) {
      throw new Error(`unknown option ${argument}\n\n${HELP}`);
    }

    const key = flag.slice(2) as keyof ParsedArguments['values'];
    if (values[key] !== undefined) {
      throw new Error(`option ${flag} was provided more than once`);
    }
    const value = inlineValue ?? argv[++index];
    if (value === undefined || value === '' || value.startsWith('--')) {
      throw new Error(`option ${flag} requires a path`);
    }
    values[key] = value;
  }

  return { help, values };
}

function isPathFlag(value: string): value is `--${keyof ParsedArguments['values']}` {
  return (
    value === '--cwd' ||
    value === '--manifest' ||
    value === '--commands' ||
    value === '--operations' ||
    value === '--policies'
  );
}

function resolvePaths(
  values: ParsedArguments['values'],
  invocationCwd: string
): CliPaths {
  const cwd = path.resolve(invocationCwd, values.cwd ?? '.');
  return {
    cwd,
    manifest: resolveFrom(cwd, values.manifest ?? DEFAULT_PATHS.manifest),
    commands: resolveFrom(cwd, values.commands ?? DEFAULT_PATHS.commands),
    operations: resolveFrom(cwd, values.operations ?? DEFAULT_PATHS.operations),
    policies: resolveFrom(cwd, values.policies ?? DEFAULT_PATHS.policies)
  };
}

function resolveFrom(cwd: string, value: string): string {
  return path.isAbsolute(value) ? path.normalize(value) : path.resolve(cwd, value);
}

function assertDistinctPaths(paths: CliPaths): void {
  const entries = [
    ['manifest', paths.manifest],
    ['commands', paths.commands],
    ['operations', paths.operations],
    ['policies', paths.policies]
  ] as const;
  const seen = new Map<string, string>();
  for (const [name, file] of entries) {
    const previous = seen.get(file);
    if (previous) {
      throw new Error(`${name} and ${previous} resolve to the same path: ${file}`);
    }
    seen.set(file, name);
  }
}

function relativeNodeImport(fromFile: string, toFile: string): string {
  let relative = path.relative(path.dirname(fromFile), toFile).split(path.sep).join('/');
  relative = relative.replace(/\.mts$/i, '.mjs').replace(/\.cts$/i, '.cjs');
  relative = relative.replace(/\.tsx?$/i, '.js');
  if (!/\.(?:c|m)?js$/i.test(relative)) relative += '.js';
  return relative.startsWith('.') ? relative : `./${relative}`;
}

function writeGeneratedFile(file: string, contents: string): void {
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, contents, 'utf8');
  } catch (error) {
    throw new Error(`failed to write ${file}: ${errorMessage(error)}`);
  }
}

function displayPath(cwd: string, file: string): string {
  const relative = path.relative(cwd, file);
  return relative && !relative.startsWith('..') ? relative : file;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const isMain =
  process.argv[1] !== undefined &&
  realPath(process.argv[1]) === realPath(fileURLToPath(import.meta.url));

if (isMain) {
  try {
    runCommandCodegenCli();
  } catch (error) {
    process.stderr.write(`distributed-gen-commands: ${errorMessage(error)}\n`);
    process.exitCode = 1;
  }
}

function realPath(file: string): string {
  try {
    return fs.realpathSync(file);
  } catch {
    return path.resolve(file);
  }
}
