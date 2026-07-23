import { createHash } from 'node:crypto';
import { Kind, parse, type OperationDefinitionNode } from 'graphql';

export type CommandManifestField = {
  name: string;
  type_name: string;
  nullable: boolean;
  list: boolean;
  item_nullable?: boolean;
  codec?: string;
  nested?: CommandManifestType;
};

export type CommandManifestType = {
  name: string;
  fields: CommandManifestField[];
};

export type CommandResultKind = 'ack' | 'fact' | 'projection' | 'none';
export type CommandReconcileKind =
  | 'subscription'
  | 'refetch'
  | 'invalidate'
  | 'none';

export type CommandManifestEntry = {
  command_name: string;
  field_name: string;
  roles: string[];
  input?: CommandManifestType;
  output?: CommandManifestType;
  input_json?: true;
  output_json?: true;
  client_reconcile?: {
    result?: { kind: CommandResultKind };
    reconcile?: { kind: CommandReconcileKind };
  };
  /** Exact generated operation and causal metadata from client manifest v4. */
  operation?: string;
  operation_hash?: string;
  causal?: {
    consistency: 'accepted' | 'fact' | 'projected';
    projects: boolean;
  };
};

export type CommandManifestV1 = {
  version: 1;
  commands: CommandManifestEntry[];
};

export type ClientProtocolOperation = {
  name: string;
  operation: string;
  operation_hash: string;
};

export type ClientCommandShapeV4 =
  | { kind: 'none' }
  | { kind: 'json'; codec: string }
  | { kind: 'object'; definition: CommandManifestType };

export type ClientCommandV4 = {
  version: 1;
  name: string;
  mutation_field: string;
  grants: string[];
  input: ClientCommandShapeV4;
  output: ClientCommandShapeV4;
  operation: string;
  operation_hash: string;
  extensions: {
    version: 2;
    consistency: {
      version: 1;
      kind: 'accepted' | 'fact' | 'projected';
    };
    confirmations?: {
      version: 1;
      kind: 'finite' | 'unavailable';
      expected: unknown[];
      fallback: 'revalidate';
    };
  };
};

/** Minimal role-selected client-manifest v4 surface consumed by command codegen. */
export type DistributedClientManifestV4 = {
  manifest_version: 4;
  protocol_version: 2;
  schema_fingerprint: string;
  capabilities: {
    causal_receipts: boolean;
  };
  commands: ClientCommandV4[];
  protocol_operations: {
    version: 1;
    command_status?: ClientProtocolOperation;
  };
};

export type CommandCodegenManifest =
  | CommandManifestV1
  | DistributedClientManifestV4;

export type GeneratedCommandArtifacts = {
  commands: string;
  operations: string;
  policies: string;
};

export type GeneratePoliciesOptions = {
  /** NodeNext import from the generated policies module to the commands module. */
  commandsImport?: string;
};

const GRAPHQL_NAME = /^[_A-Za-z][_0-9A-Za-z]*$/;
const SHA256 = /^sha256:[0-9a-f]{64}$/;
const MAX_GENERATED_OPERATION_BYTES = 1024 * 1024;
const MAX_V4_COMMANDS = 4_096;
const MAX_V4_FIELDS = 4_096;
const MAX_V4_TYPE_DEPTH = 64;
const TYPESCRIPT_RESERVED = new Set([
  'any',
  'as',
  'asserts',
  'async',
  'await',
  'bigint',
  'boolean',
  'break',
  'case',
  'catch',
  'class',
  'const',
  'constructor',
  'continue',
  'debugger',
  'declare',
  'default',
  'delete',
  'do',
  'else',
  'enum',
  'export',
  'extends',
  'false',
  'finally',
  'for',
  'from',
  'function',
  'get',
  'if',
  'implements',
  'import',
  'in',
  'infer',
  'instanceof',
  'interface',
  'is',
  'keyof',
  'let',
  'module',
  'namespace',
  'never',
  'new',
  'null',
  'number',
  'object',
  'of',
  'package',
  'private',
  'protected',
  'public',
  'readonly',
  'require',
  'return',
  'set',
  'static',
  'string',
  'super',
  'switch',
  'symbol',
  'this',
  'throw',
  'true',
  'try',
  'type',
  'typeof',
  'undefined',
  'unique',
  'unknown',
  'var',
  'void',
  'while',
  'with',
  'yield'
]);
const RESULT_KINDS = new Set<CommandResultKind>([
  'ack',
  'fact',
  'projection',
  'none'
]);
const RECONCILE_KINDS = new Set<CommandReconcileKind>([
  'subscription',
  'refetch',
  'invalidate',
  'none'
]);
const MANIFEST_V1_SCALAR_TYPES = new Set([
  'String',
  'ID',
  'Boolean',
  'Int',
  'Float',
  'BigInt',
  'JSON'
]);

/** Validate and normalize a Distributed command manifest (format version 1). */
export function parseCommandManifest(value: unknown): CommandManifestV1 {
  const root = expectRecord(value, 'manifest');
  if (root.version !== 1) {
    throw new Error('invalid command manifest: expected version 1');
  }
  if (!Array.isArray(root.commands)) {
    throw new Error('invalid command manifest: commands must be an array');
  }

  const commands = root.commands.map((entry, index) => parseCommand(entry, index));
  const fields = new Set<string>();
  const functions = new Set<string>();
  for (const command of commands) {
    if (fields.has(command.field_name)) {
      throw new Error(`invalid command manifest: duplicate field ${command.field_name}`);
    }
    fields.add(command.field_name);

    const functionName = fieldToFunctionName(command.field_name);
    if (TYPESCRIPT_RESERVED.has(functionName)) {
      throw new Error(
        `invalid command manifest: field ${command.field_name} maps to reserved TypeScript name ${functionName}`
      );
    }
    if (functions.has(functionName)) {
      throw new Error(
        `invalid command manifest: fields collide at function ${functionName}`
      );
    }
    functions.add(functionName);
  }
  assertCompatibleTypeDefinitions(commands);

  return { version: 1, commands };
}

/**
 * Validate either the legacy command-only manifest or the role-selected v4
 * client manifest. V4 preserves compiler-owned operation bytes verbatim.
 */
export function parseCodegenManifest(value: unknown): CommandCodegenManifest {
  const root = expectRecord(value, 'manifest');
  if (root.version === 1) return parseCommandManifest(value);
  if (root.manifest_version !== 4) {
    throw new Error(
      'invalid command manifest: expected legacy version 1 or client manifest_version 4'
    );
  }
  if (root.protocol_version !== 2) {
    throw new Error('invalid command manifest: expected protocol_version 2');
  }
  const schemaFingerprint = expectHash(
    root.schema_fingerprint,
    'manifest.schema_fingerprint'
  );
  const capabilities = expectRecord(
    root.capabilities,
    'manifest.capabilities'
  );
  const causalReceipts = expectBoolean(
    capabilities.causal_receipts,
    'manifest.capabilities.causal_receipts'
  );
  if (!Array.isArray(root.commands)) {
    throw new Error('invalid command manifest: commands must be an array');
  }
  if (root.commands.length > MAX_V4_COMMANDS) {
    throw new Error('invalid command manifest: commands exceeds the supported bound');
  }

  const commands = root.commands.map((entry, index) =>
    parseClientCommandV4(entry, index)
  );
  validateCommandIdentities(normalizeManifestCommands({ commands }));

  const operations = expectRecord(
    root.protocol_operations,
    'manifest.protocol_operations'
  );
  if (operations.version !== 1) {
    throw new Error(
      'invalid command manifest: manifest.protocol_operations.version must be 1'
    );
  }
  const commandStatus =
    operations.command_status === undefined ||
    operations.command_status === null
      ? undefined
      : parseProtocolOperation(
          operations.command_status,
          'manifest.protocol_operations.command_status',
          'query'
        );
  if (commands.length > 0 && !causalReceipts) {
    throw new Error(
      'invalid command manifest: generated commands require causal_receipts'
    );
  }
  if (commands.length > 0 && commandStatus === undefined) {
    throw new Error(
      'invalid command manifest: generated commands require command_status'
    );
  }

  return {
    manifest_version: 4,
    protocol_version: 2,
    schema_fingerprint: schemaFingerprint,
    capabilities: { causal_receipts: causalReceipts },
    commands,
    protocol_operations: {
      version: 1,
      ...(commandStatus ? { command_status: commandStatus } : {})
    }
  };
}

/** Build one mutation document from a normalized command entry. */
export function buildMutationOperation(command: CommandManifestEntry): string {
  if (command.operation !== undefined) return command.operation;
  const operationName = `Command_${command.field_name}`;
  const selection = command.output?.fields.map((field) => field.name) ?? [];
  const fieldStart = command.input
    ? `  ${command.field_name}(input: $input)`
    : `  ${command.field_name}`;
  const fieldLines = selection.length
    ? [
        `${fieldStart} {`,
        ...selection.map((field) => `    ${field}`),
        '  }'
      ]
    : [fieldStart];

  return command.input
    ? [
        `mutation ${operationName}($input: ${command.input.name}!) {`,
        ...fieldLines,
        '}'
      ].join('\n')
    : [`mutation ${operationName} {`, ...fieldLines, '}'].join('\n');
}

/** Generate copy-pasteable GraphQL operations for the whole manifest. */
export function generateOperationsGql(value: CommandCodegenManifest | unknown): string {
  const manifest = parseCodegenManifest(value);
  const commands = normalizeManifestCommands(manifest);
  const lines = [
    '# GENERATED by @hops-ops/distributed — do not edit by hand.',
    '# Source: commands.manifest.json',
    '# Regenerate with: distributed-gen-commands',
    ''
  ];

  for (const command of commands) {
    const roles = command.roles.length
      ? command.roles.map((role) => JSON.stringify(role)).join(', ')
      : '(any)';
    lines.push(`# ${JSON.stringify(command.command_name)} — roles: ${roles}`);
    lines.push(buildMutationOperation(command));
    lines.push('');
  }
  if (isClientManifestV4(manifest) && manifest.protocol_operations.command_status) {
    lines.push('# Framework-owned causal receipt status operation.');
    lines.push(manifest.protocol_operations.command_status.operation);
    lines.push('');
  }

  return withFinalNewline(lines.join('\n'));
}

/** Generate typed descriptors, standalone calls, and a single bound command API. */
export function generateCommandsTs(value: CommandCodegenManifest | unknown): string {
  const manifest = parseCodegenManifest(value);
  const commands = normalizeManifestCommands(manifest);
  const causal = isClientManifestV4(manifest);
  const lines: string[] = [
    '/**',
    ' * GENERATED by @hops-ops/distributed — do not edit by hand.',
    ' * Source: commands.manifest.json',
    ' * Regenerate with: distributed-gen-commands',
    ' */',
    'import {',
    '  bindCommands as bindDistributedCommands,',
    '  defineCommand,',
    '  defineCommands,',
    ...(causal ? ['  defineCausalProtocol,'] : []),
    '  executeCommand,',
    '  type BindCommandsOptions,',
    '  type BoundCommandSet,',
    '  type CommandCallOptions,',
    '  type CommandClient,',
    '  type GqlResult',
    "} from '@hops-ops/distributed';",
    ''
  ];

  if (causal) {
    const status = manifest.protocol_operations.command_status!;
    lines.push(
      '/** Compiler-owned causal protocol operations. Never synthesized by the client. */',
      'export const COMMAND_PROTOCOL = defineCausalProtocol({',
      '  protocolVersion: 2,',
      `  schemaHash: ${JSON.stringify(manifest.schema_fingerprint)},`,
      '  commandStatus: {',
      `    name: ${JSON.stringify(status.name)},`,
      `    document: ${JSON.stringify(status.operation)},`,
      `    operationHash: ${JSON.stringify(status.operation_hash)}`,
      '  }',
      '});',
      ''
    );
  }

  for (const definition of collectTypeDefinitions(commands)) {
    lines.push(`export type ${definition.name} = {`);
    for (const field of definition.fields) {
      const optional = field.nullable ? '?' : '';
      const nullable = field.nullable ? ' | null' : '';
      lines.push(
        `  ${field.name}${optional}: ${manifestFieldToTs(field)}${nullable};`
      );
    }
    lines.push('};', '');
  }

  lines.push(
    '/** Field name to engine roles. Client metadata is not an authorization boundary. */',
    'export const COMMAND_ROLES = {'
  );
  for (const command of commands) {
    const roles = command.roles.map((role) => JSON.stringify(role)).join(', ');
    lines.push(`  ${JSON.stringify(command.field_name)}: [${roles}] as const,`);
  }
  lines.push('} as const;', '');

  lines.push(
    '/** Mutation documents mirrored in commands.operations.gql. */',
    'export const COMMAND_DOCS = {'
  );
  for (const command of commands) {
    lines.push(
      `  ${JSON.stringify(command.field_name)}: ${JSON.stringify(buildMutationOperation(command))},`
    );
  }
  lines.push('} as const;', '');

  lines.push('/** Typed command descriptors consumed by the package runtime. */');
  lines.push('export const COMMANDS = defineCommands({');
  for (const command of commands) {
    const functionName = fieldToFunctionName(command.field_name);
    const inputType = commandInputType(command);
    const outputType = commandOutputType(command);
    lines.push(
      `  ${functionName}: defineCommand<${inputType}, ${outputType}>({`,
      `    field: ${JSON.stringify(command.field_name)},`,
      `    document: COMMAND_DOCS[${JSON.stringify(command.field_name)}],`,
      `    hasInput: ${commandHasInput(command) ? 'true' : 'false'},`,
      `    roles: COMMAND_ROLES[${JSON.stringify(command.field_name)}],`,
      ...(command.causal
        ? [
            '    causal: {',
            '      protocol: COMMAND_PROTOCOL,',
            `      operationHash: ${JSON.stringify(command.operation_hash)},`,
            `      projects: ${command.causal.projects ? 'true' : 'false'}`,
            '    }'
          ]
        : []),
      '  }),'
    );
  }
  lines.push('});', '');

  for (const command of commands) {
    const functionName = fieldToFunctionName(command.field_name);
    const outputType = commandOutputType(command);
    lines.push('/**');
    lines.push(
      ` * ${commentLine(command.command_name)} -> GraphQL \`${command.field_name}\`.`
    );
    lines.push(' */');
    if (commandHasInput(command)) {
      lines.push(
        `export function ${functionName}(input: ${commandInputType(command)}, client: CommandClient, options?: CommandCallOptions): Promise<GqlResult<${outputType}>> {`,
        `  return executeCommand(client, COMMANDS.${functionName}, input, options);`,
        '}',
        ''
      );
    } else {
      lines.push(
        `export function ${functionName}(client: CommandClient, options?: CommandCallOptions): Promise<GqlResult<${outputType}>> {`,
        `  return executeCommand(client, COMMANDS.${functionName}, options);`,
        '}',
        ''
      );
    }
  }

  lines.push(
    '/** Commands pre-bound to a GraphQL client, with optional per-call policies. */',
    'export type BoundCommands = BoundCommandSet<typeof COMMANDS>;',
    'export type CommandBindOptions = BindCommandsOptions<typeof COMMANDS>;',
    '',
    '/** Bind URL/auth/cache once, then call `commands.someCommand(input, options?)`. */',
    'export function bindCommands(',
    '  client: CommandClient,',
    '  options: CommandBindOptions = {}',
    '): BoundCommands {',
    '  return bindDistributedCommands(client, COMMANDS, options);',
    '}',
    ''
  );

  return withFinalNewline(lines.join('\n'));
}

/** Generate default client policies from manifest reconciliation hints. */
export function generateCommandPoliciesTs(
  value: CommandCodegenManifest | unknown,
  options: GeneratePoliciesOptions = {}
): string {
  const manifest = parseCodegenManifest(value);
  const commands = normalizeManifestCommands(manifest);
  const commandsImport = options.commandsImport ?? './commands.generated.js';
  if (!isRelativeNodeImport(commandsImport)) {
    throw new Error(
      'commandsImport must be a relative NodeNext specifier ending in .js, .mjs, or .cjs'
    );
  }

  const lines: string[] = [
    '/**',
    ' * GENERATED by @hops-ops/distributed — do not edit by hand.',
    ' * Source: commands.manifest.json client_reconcile hints',
    ' * Regenerate with: distributed-gen-commands',
    ' */',
    "import type { CommandPolicyMap } from '@hops-ops/distributed';",
    `import type { COMMANDS } from ${JSON.stringify(commandsImport)};`,
    '',
    '/** Call-site options override these generated defaults. */',
    'export const commandPolicies = {'
  ];

  for (const command of commands) {
    const client = command.client_reconcile;
    if (!client?.result && !client?.reconcile) continue;
    const functionName = fieldToFunctionName(command.field_name);
    lines.push(`  ${functionName}: {`);
    if (client.result) {
      lines.push(`    result: { kind: ${JSON.stringify(client.result.kind)} },`);
    }
    if (client.reconcile) {
      lines.push(
        `    reconcile: { kind: ${JSON.stringify(client.reconcile.kind)} },`
      );
    }
    lines.push('  },');
  }

  lines.push('} satisfies CommandPolicyMap<typeof COMMANDS>;', '');
  return withFinalNewline(lines.join('\n'));
}

/** Generate all three app-owned artifacts from one validated manifest. */
export function generateCommandArtifacts(
  value: CommandCodegenManifest | unknown,
  options: GeneratePoliciesOptions = {}
): GeneratedCommandArtifacts {
  const manifest = parseCodegenManifest(value);
  return {
    commands: generateCommandsTs(manifest),
    operations: generateOperationsGql(manifest),
    policies: generateCommandPoliciesTs(manifest, options)
  };
}

/** Convert a GraphQL field name such as `todos_create` to `todosCreate`. */
export function fieldToFunctionName(field: string): string {
  return field.replace(/_([A-Za-z0-9])/g, (_match, character: string) =>
    character.toUpperCase()
  );
}

function isClientManifestV4(
  manifest: CommandCodegenManifest
): manifest is DistributedClientManifestV4 {
  return 'manifest_version' in manifest;
}

function normalizeManifestCommands(
  manifest: Pick<DistributedClientManifestV4, 'commands'> | CommandManifestV1
): CommandManifestEntry[] {
  if ('version' in manifest) return manifest.commands;
  return manifest.commands.map((command) => {
    const confirmations = command.extensions.confirmations;
    const consistency = command.extensions.consistency.kind;
    const projects =
      consistency === 'fact' ||
      consistency === 'projected' ||
      (consistency === 'accepted' &&
        confirmations?.kind === 'finite' &&
        confirmations.expected.length > 0);
    return {
      command_name: command.name,
      field_name: command.mutation_field,
      roles: command.grants,
      ...(command.input.kind === 'object'
        ? { input: command.input.definition }
        : command.input.kind === 'json'
          ? { input_json: true as const }
          : {}),
      ...(command.output.kind === 'object'
        ? { output: command.output.definition }
        : command.output.kind === 'json'
          ? { output_json: true as const }
          : {}),
      operation: command.operation,
      operation_hash: command.operation_hash,
      causal: { consistency, projects }
    };
  });
}

function parseClientCommandV4(value: unknown, index: number): ClientCommandV4 {
  const path = `manifest.commands[${index}]`;
  const command = expectRecord(value, path);
  if (command.version !== 1) {
    throw new Error(`invalid command manifest: ${path}.version must be 1`);
  }
  const name = expectNonEmptyString(command.name, `${path}.name`);
  const mutationField = expectGraphqlName(
    command.mutation_field,
    `${path}.mutation_field`
  );
  const grants = parseRoles(command.grants, `${path}.grants`);
  const input = parseClientCommandShapeV4(command.input, `${path}.input`);
  const output = parseClientCommandShapeV4(command.output, `${path}.output`);
  const operation = expectGeneratedOperation(
    command.operation,
    `${path}.operation`
  );
  const operationHash = expectHash(
    command.operation_hash,
    `${path}.operation_hash`
  );
  assertOperationHash(operation, operationHash, `${path}.operation_hash`);
  validateGraphqlOperation(
    operation,
    `${path}.operation`,
    'mutation',
    `Client_${mutationField}`,
    mutationField
  );

  const extensions = expectRecord(command.extensions, `${path}.extensions`);
  if (extensions.version !== 2) {
    throw new Error(`invalid command manifest: ${path}.extensions.version must be 2`);
  }
  const consistencyRecord = expectRecord(
    extensions.consistency,
    `${path}.extensions.consistency`
  );
  if (consistencyRecord.version !== 1) {
    throw new Error(
      `invalid command manifest: ${path}.extensions.consistency.version must be 1`
    );
  }
  const consistency = expectEnum(
    consistencyRecord.kind,
    ['accepted', 'fact', 'projected'] as const,
    `${path}.extensions.consistency.kind`
  );
  const confirmations =
    extensions.confirmations === undefined ||
    extensions.confirmations === null
      ? undefined
      : parseConfirmationsV4(
          extensions.confirmations,
          `${path}.extensions.confirmations`
        );

  return {
    version: 1,
    name,
    mutation_field: mutationField,
    grants,
    input,
    output,
    operation,
    operation_hash: operationHash,
    extensions: {
      version: 2,
      consistency: { version: 1, kind: consistency },
      ...(confirmations ? { confirmations } : {})
    }
  };
}

function parseClientCommandShapeV4(
  value: unknown,
  path: string
): ClientCommandShapeV4 {
  const shape = expectRecord(value, path);
  const kind = expectEnum(
    shape.kind,
    ['none', 'json', 'object'] as const,
    `${path}.kind`
  );
  switch (kind) {
    case 'none':
      return { kind };
    case 'json':
      return {
        kind,
        codec: expectNonEmptyString(shape.codec, `${path}.codec`)
      };
    case 'object':
      return {
        kind,
        definition: parseManifestTypeV4(
          shape.definition,
          `${path}.definition`,
          0
        )
      };
  }
}

function parseManifestTypeV4(
  value: unknown,
  path: string,
  depth: number
): CommandManifestType {
  if (depth > MAX_V4_TYPE_DEPTH) {
    throw new Error(`invalid command manifest: ${path} exceeds the type-depth bound`);
  }
  const type = expectRecord(value, path);
  const name = expectGraphqlName(type.name, `${path}.name`);
  if (TYPESCRIPT_RESERVED.has(name)) {
    throw new Error(`invalid command manifest: ${path}.name is reserved in TypeScript`);
  }
  if (!Array.isArray(type.fields) || type.fields.length > MAX_V4_FIELDS) {
    throw new Error(
      `invalid command manifest: ${path}.fields must be a bounded array`
    );
  }
  const names = new Set<string>();
  const fields = type.fields.map((field, index): CommandManifestField => {
    const fieldPath = `${path}.fields[${index}]`;
    const record = expectRecord(field, fieldPath);
    const fieldName = expectGraphqlName(record.name, `${fieldPath}.name`);
    if (names.has(fieldName)) {
      throw new Error(`invalid command manifest: duplicate field ${path}.${fieldName}`);
    }
    names.add(fieldName);
    const typeName = expectGraphqlName(record.type_name, `${fieldPath}.type_name`);
    const nullable = expectBoolean(record.nullable, `${fieldPath}.nullable`);
    const list = expectBoolean(record.list, `${fieldPath}.list`);
    const itemNullable = expectBoolean(
      record.item_nullable,
      `${fieldPath}.item_nullable`
    );
    if (!list && itemNullable) {
      throw new Error(
        `invalid command manifest: ${fieldPath}.item_nullable requires a list`
      );
    }
    const codec =
      record.codec === undefined || record.codec === null
        ? undefined
        : expectNonEmptyString(record.codec, `${fieldPath}.codec`);
    const nested =
      record.nested === undefined || record.nested === null
        ? undefined
        : parseManifestTypeV4(record.nested, `${fieldPath}.nested`, depth + 1);
    if ((codec === undefined) === (nested === undefined)) {
      throw new Error(
        `invalid command manifest: ${fieldPath} must declare exactly one codec or nested type`
      );
    }
    if (nested && nested.name !== typeName) {
      throw new Error(
        `invalid command manifest: ${fieldPath}.nested.name must match type_name`
      );
    }
    return {
      name: fieldName,
      type_name: typeName,
      nullable,
      list,
      item_nullable: itemNullable,
      ...(codec ? { codec } : {}),
      ...(nested ? { nested } : {})
    };
  });
  return { name, fields };
}

function parseConfirmationsV4(
  value: unknown,
  path: string
): NonNullable<ClientCommandV4['extensions']['confirmations']> {
  const confirmations = expectRecord(value, path);
  if (confirmations.version !== 1) {
    throw new Error(`invalid command manifest: ${path}.version must be 1`);
  }
  const kind = expectEnum(
    confirmations.kind,
    ['finite', 'unavailable'] as const,
    `${path}.kind`
  );
  if (
    !Array.isArray(confirmations.expected) ||
    confirmations.expected.length > MAX_V4_FIELDS
  ) {
    throw new Error(`invalid command manifest: ${path}.expected must be a bounded array`);
  }
  if (kind === 'unavailable' && confirmations.expected.length !== 0) {
    throw new Error(
      `invalid command manifest: ${path}.expected must be empty when unavailable`
    );
  }
  const fallback = expectEnum(
    confirmations.fallback,
    ['revalidate'] as const,
    `${path}.fallback`
  );
  return {
    version: 1,
    kind,
    expected: [...confirmations.expected],
    fallback
  };
}

function parseProtocolOperation(
  value: unknown,
  path: string,
  expectedKind: 'query'
): ClientProtocolOperation {
  const operation = expectRecord(value, path);
  const name = expectGraphqlName(operation.name, `${path}.name`);
  const source = expectGeneratedOperation(operation.operation, `${path}.operation`);
  const hash = expectHash(operation.operation_hash, `${path}.operation_hash`);
  assertOperationHash(source, hash, `${path}.operation_hash`);
  validateGraphqlOperation(
    source,
    `${path}.operation`,
    expectedKind,
    name,
    'commandStatus'
  );
  return { name, operation: source, operation_hash: hash };
}

function validateGraphqlOperation(
  source: string,
  path: string,
  expectedKind: 'query' | 'mutation',
  expectedName: string,
  expectedRootField: string
): void {
  let operation: OperationDefinitionNode;
  try {
    const document = parse(source);
    if (
      document.definitions.length !== 1 ||
      document.definitions[0]?.kind !== Kind.OPERATION_DEFINITION
    ) {
      throw new Error('expected one operation');
    }
    operation = document.definitions[0];
  } catch {
    throw new Error(`invalid command manifest: ${path} must be one GraphQL operation`);
  }
  if (
    operation.operation !== expectedKind ||
    operation.name?.value !== expectedName ||
    operation.selectionSet.selections.length !== 1 ||
    operation.selectionSet.selections[0]?.kind !== Kind.FIELD ||
    operation.selectionSet.selections[0].name.value !== expectedRootField
  ) {
    throw new Error(
      `invalid command manifest: ${path} does not match its generated descriptor`
    );
  }
  const commandId = operation.variableDefinitions?.find(
    (definition) => definition.variable.name.value === 'commandId'
  );
  if (
    commandId?.type.kind !== Kind.NON_NULL_TYPE ||
    commandId.type.type.kind !== Kind.NAMED_TYPE ||
    commandId.type.type.name.value !== 'ID'
  ) {
    throw new Error(
      `invalid command manifest: ${path} must require $commandId: ID!`
    );
  }
  const root = operation.selectionSet.selections[0];
  const argument = root.arguments?.find((item) => item.name.value === 'commandId');
  if (
    argument?.value.kind !== Kind.VARIABLE ||
    argument.value.name.value !== 'commandId'
  ) {
    throw new Error(
      `invalid command manifest: ${path} must pass the generated commandId`
    );
  }
}

function expectGeneratedOperation(value: unknown, path: string): string {
  const operation = expectNonEmptyString(value, path);
  if (new TextEncoder().encode(operation).length > MAX_GENERATED_OPERATION_BYTES) {
    throw new Error(`invalid command manifest: ${path} exceeds the operation bound`);
  }
  return operation;
}

function expectHash(value: unknown, path: string): string {
  const hash = expectNonEmptyString(value, path);
  if (!SHA256.test(hash)) {
    throw new Error(`invalid command manifest: ${path} must be a canonical SHA-256`);
  }
  return hash;
}

function assertOperationHash(
  operation: string,
  expected: string,
  path: string
): void {
  const actual = `sha256:${createHash('sha256').update(operation).digest('hex')}`;
  if (actual !== expected) {
    throw new Error(`invalid command manifest: ${path} does not match operation bytes`);
  }
}

function expectEnum<const T extends readonly string[]>(
  value: unknown,
  values: T,
  path: string
): T[number] {
  if (typeof value !== 'string' || !values.includes(value)) {
    throw new Error(
      `invalid command manifest: ${path} must be one of ${values.join(', ')}`
    );
  }
  return value as T[number];
}

function validateCommandIdentities(commands: CommandManifestEntry[]): void {
  const fields = new Set<string>();
  const functions = new Set<string>();
  for (const command of commands) {
    if (fields.has(command.field_name)) {
      throw new Error(`invalid command manifest: duplicate field ${command.field_name}`);
    }
    fields.add(command.field_name);
    const functionName = fieldToFunctionName(command.field_name);
    if (TYPESCRIPT_RESERVED.has(functionName)) {
      throw new Error(
        `invalid command manifest: field ${command.field_name} maps to reserved TypeScript name ${functionName}`
      );
    }
    if (functions.has(functionName)) {
      throw new Error(
        `invalid command manifest: fields collide at function ${functionName}`
      );
    }
    functions.add(functionName);
  }
  assertCompatibleTypeDefinitions(commands);
}

function parseCommand(value: unknown, index: number): CommandManifestEntry {
  const path = `manifest.commands[${index}]`;
  const command = expectRecord(value, path);
  const commandName = expectNonEmptyString(command.command_name, `${path}.command_name`);
  const fieldName = expectGraphqlName(command.field_name, `${path}.field_name`);
  const roles = command.roles === undefined ? [] : parseRoles(command.roles, `${path}.roles`);
  const input =
    command.input === null || command.input === undefined
      ? undefined
      : parseManifestType(command.input, `${path}.input`);
  const output =
    command.output === null || command.output === undefined
      ? undefined
      : parseManifestType(command.output, `${path}.output`);
  const clientReconcile =
    command.client_reconcile === null || command.client_reconcile === undefined
      ? undefined
      : parseClientReconcile(command.client_reconcile, `${path}.client_reconcile`);

  return {
    command_name: commandName,
    field_name: fieldName,
    roles,
    ...(input ? { input } : {}),
    ...(output ? { output } : {}),
    ...(clientReconcile ? { client_reconcile: clientReconcile } : {})
  };
}

function parseManifestType(value: unknown, path: string): CommandManifestType {
  const type = expectRecord(value, path);
  const name = expectGraphqlName(type.name, `${path}.name`);
  if (TYPESCRIPT_RESERVED.has(name)) {
    throw new Error(`invalid command manifest: ${path}.name is reserved in TypeScript`);
  }
  if (!Array.isArray(type.fields)) {
    throw new Error(`invalid command manifest: ${path}.fields must be an array`);
  }

  const names = new Set<string>();
  const fields = type.fields.map((field, index) => {
    const fieldPath = `${path}.fields[${index}]`;
    const record = expectRecord(field, fieldPath);
    const fieldName = expectGraphqlName(record.name, `${fieldPath}.name`);
    if (names.has(fieldName)) {
      throw new Error(`invalid command manifest: duplicate field ${path}.${fieldName}`);
    }
    names.add(fieldName);
    const typeName = expectGraphqlName(record.type_name, `${fieldPath}.type_name`);
    if (!MANIFEST_V1_SCALAR_TYPES.has(typeName)) {
      throw new Error(
        `invalid command manifest: ${fieldPath}.type_name ${JSON.stringify(typeName)} is unsupported; manifest version 1 can only generate scalar fields (${[...MANIFEST_V1_SCALAR_TYPES].join(', ')}). Nested objects, enums, and custom scalars require explicit type metadata in a future manifest version`
      );
    }
    return {
      name: fieldName,
      type_name: typeName,
      nullable: expectBoolean(record.nullable, `${fieldPath}.nullable`),
      list: expectBoolean(record.list, `${fieldPath}.list`)
    };
  });

  return { name, fields };
}

function parseRoles(value: unknown, path: string): string[] {
  if (!Array.isArray(value)) {
    throw new Error(`invalid command manifest: ${path} must be an array`);
  }
  return value.map((role, index) =>
    expectNonEmptyString(role, `${path}[${index}]`)
  );
}

function parseClientReconcile(
  value: unknown,
  path: string
): NonNullable<CommandManifestEntry['client_reconcile']> {
  const client = expectRecord(value, path);
  let result: { kind: CommandResultKind } | undefined;
  let reconcile: { kind: CommandReconcileKind } | undefined;

  if (client.result !== null && client.result !== undefined) {
    const record = expectRecord(client.result, `${path}.result`);
    const kind = expectNonEmptyString(record.kind, `${path}.result.kind`);
    if (!RESULT_KINDS.has(kind as CommandResultKind)) {
      throw new Error(`invalid command manifest: unsupported result kind ${kind}`);
    }
    result = { kind: kind as CommandResultKind };
  }

  if (client.reconcile !== null && client.reconcile !== undefined) {
    const record = expectRecord(client.reconcile, `${path}.reconcile`);
    const kind = expectNonEmptyString(record.kind, `${path}.reconcile.kind`);
    if (!RECONCILE_KINDS.has(kind as CommandReconcileKind)) {
      throw new Error(`invalid command manifest: unsupported reconcile kind ${kind}`);
    }
    reconcile = { kind: kind as CommandReconcileKind };
  }

  return {
    ...(result ? { result } : {}),
    ...(reconcile ? { reconcile } : {})
  };
}

function collectTypeDefinitions(commands: CommandManifestEntry[]): CommandManifestType[] {
  const types = new Map<string, CommandManifestType>();
  for (const definition of collectAllTypeDefinitions(commands)) {
    if (!types.has(definition.name)) types.set(definition.name, definition);
  }
  return [...types.values()];
}

function assertCompatibleTypeDefinitions(commands: CommandManifestEntry[]): void {
  const definitions = new Map<string, string>();
  for (const type of collectAllTypeDefinitions(commands)) {
    const shape = JSON.stringify(type.fields);
    const existing = definitions.get(type.name);
    if (existing !== undefined && existing !== shape) {
      throw new Error(
        `invalid command manifest: conflicting definitions for type ${type.name}`
      );
    }
    definitions.set(type.name, shape);
  }
}

function collectAllTypeDefinitions(commands: CommandManifestEntry[]): CommandManifestType[] {
  const definitions: CommandManifestType[] = [];
  const visit = (definition: CommandManifestType | undefined): void => {
    if (!definition) return;
    definitions.push(definition);
    for (const field of definition.fields) visit(field.nested);
  };
  for (const command of commands) {
    visit(command.input);
    visit(command.output);
  }
  return definitions;
}

function manifestFieldToTs(field: CommandManifestField): string {
  let type: string;
  if (field.nested) {
    type = field.nested.name;
  } else if (field.codec) {
    switch (field.codec) {
      case 'string':
      case 'string_unvalidated_timestamp':
      case 'base64':
        type = 'string';
        break;
      case 'boolean':
        type = 'boolean';
        break;
      case 'int32':
      case 'float64':
      case 'json_number_precision_limited':
        type = 'number';
        break;
      case 'json':
      default:
        type = 'unknown';
    }
  } else {
  switch (field.type_name) {
    case 'String':
    case 'ID':
      type = 'string';
      break;
    case 'Boolean':
      type = 'boolean';
      break;
    case 'Int':
    case 'Float':
    case 'BigInt':
      type = 'number';
      break;
    case 'JSON':
      type = 'unknown';
      break;
    default:
      type = field.type_name;
  }
  }
  const item = field.item_nullable ? `${type} | null` : type;
  return field.list ? `Array<${item}>` : type;
}

function commandHasInput(command: CommandManifestEntry): boolean {
  return command.input !== undefined || command.input_json === true;
}

function commandInputType(command: CommandManifestEntry): string {
  if (command.input) return command.input.name;
  return command.input_json ? 'unknown' : 'void';
}

function commandOutputType(command: CommandManifestEntry): string {
  if (command.output) return command.output.name;
  return 'unknown';
}

function expectRecord(value: unknown, path: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`invalid command manifest: ${path} must be an object`);
  }
  return value as Record<string, unknown>;
}

function expectNonEmptyString(value: unknown, path: string): string {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(`invalid command manifest: ${path} must be a non-empty string`);
  }
  return value;
}

function expectGraphqlName(value: unknown, path: string): string {
  const name = expectNonEmptyString(value, path);
  if (!GRAPHQL_NAME.test(name)) {
    throw new Error(`invalid command manifest: ${path} is not a GraphQL name`);
  }
  return name;
}

function expectBoolean(value: unknown, path: string): boolean {
  if (typeof value !== 'boolean') {
    throw new Error(`invalid command manifest: ${path} must be a boolean`);
  }
  return value;
}

function commentLine(value: string): string {
  return value
    .replace(/[\r\n\u2028\u2029]+/g, ' ')
    .replace(/\*\//g, '*\\/')
    .trim();
}

function withFinalNewline(value: string): string {
  return `${value.replace(/\n+$/g, '')}\n`;
}

function isRelativeNodeImport(value: string): boolean {
  return (
    (value.startsWith('./') || value.startsWith('../')) &&
    /\.(?:c|m)?js$/.test(value)
  );
}
