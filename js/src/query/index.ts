/**
 * Typed QuerySpec authoring surface.
 *
 * GraphQL documents and QuerySpec builders are dual frontends. Builders produce
 * portable QuerySpec JSON that `dctl client` lowers into the same GraphQL
 * operation text and frozen replica artifacts as hand-written `.graphql` files.
 *
 * This module is intentionally build-time oriented: call `build()` / `toJSON()`
 * / `toGraphql()` to emit committed sources. Runtime still executes generated
 * operation artifacts only.
 */

export const QUERY_SPEC_VERSION = 1 as const;

/** GraphQL enum token. Serializes as an unquoted GraphQL name. */
export type QueryEnumValue = {
	readonly $enum: string;
};

/** GraphQL variable reference. Serializes as `$name`. */
export type QueryVariableRef = {
	readonly $var: string;
};

export type QueryValue =
	| null
	| boolean
	| number
	| string
	| QueryEnumValue
	| QueryVariableRef
	| readonly QueryValue[]
	| { readonly [key: string]: QueryValue };

/** Nested field selection. `true` selects a leaf; objects select relationships. */
export type QuerySelection = {
	readonly [field: string]: true | QuerySelection;
};

export type QuerySpecVariable = {
	readonly name: string;
	readonly type: string;
};

export type QuerySpecRoot = {
	readonly field: string;
	readonly alias?: string;
	readonly args?: Readonly<Record<string, QueryValue>>;
	readonly select: QuerySelection;
};

/**
 * Portable IR consumed by `dctl client` from `*.query.json` files.
 * Version is closed; unknown fields are rejected by the compiler.
 */
export type QuerySpec = {
	readonly version: typeof QUERY_SPEC_VERSION;
	readonly name: string;
	readonly load?: boolean;
	readonly live?: boolean;
	readonly variables?: readonly QuerySpecVariable[];
	readonly roots: readonly QuerySpecRoot[];
};

/** Mark a GraphQL enum token (e.g. order direction `asc`). */
export function gqlEnum(value: string): QueryEnumValue {
	assertIdent(value, 'enum value');
	return Object.freeze({ $enum: value });
}

/** Reference a declared operation variable. */
export function gqlVar(name: string): QueryVariableRef {
	assertIdent(name, 'variable name');
	return Object.freeze({ $var: name });
}

/** True when value is a QuerySpec enum tag. */
export function isQueryEnumValue(value: unknown): value is QueryEnumValue {
	return (
		typeof value === 'object' &&
		value !== null &&
		Object.keys(value).length === 1 &&
		typeof (value as QueryEnumValue).$enum === 'string'
	);
}

/** True when value is a QuerySpec variable tag. */
export function isQueryVariableRef(value: unknown): value is QueryVariableRef {
	return (
		typeof value === 'object' &&
		value !== null &&
		Object.keys(value).length === 1 &&
		typeof (value as QueryVariableRef).$var === 'string'
	);
}

export type OrderDirection = 'asc' | 'desc';

/**
 * Start a named query builder.
 *
 * @example
 * ```ts
 * const Todos = defineQuery('Todos')
 *   .from('todos')
 *   .orderBy({ status: 'asc' }, { todo_id: 'asc' })
 *   .select({ todo_id: true, title: true, status: true })
 *   .load()
 *   .build();
 * ```
 */
export function defineQuery(name: string): QueryBuilder {
	return new QueryBuilder(name);
}

export class QueryBuilder {
	readonly #name: string;
	#load = false;
	#live = false;
	#variables: QuerySpecVariable[] = [];
	#field: string | undefined;
	#alias: string | undefined;
	#args: Record<string, QueryValue> = {};
	#select: QuerySelection | undefined;

	constructor(name: string) {
		assertOperationName(name);
		this.#name = name;
	}

	/** Root GraphQL field (read-model collection or by-pk field). */
	from(field: string, options?: { alias?: string }): this {
		assertIdent(field, 'root field');
		this.#field = field;
		if (options?.alias !== undefined) {
			assertIdent(options.alias, 'root alias');
			this.#alias = options.alias;
		}
		return this;
	}

	/** Declare an operation variable (`$name: Type`). */
	variable(name: string, type: string): this {
		assertIdent(name, 'variable name');
		assertGraphqlType(type);
		if (this.#variables.some((entry) => entry.name === name)) {
			throw new TypeError(`QueryBuilder variable \`${name}\` is already declared`);
		}
		this.#variables.push(Object.freeze({ name, type }));
		return this;
	}

	/** Merge root field arguments. Later keys overwrite earlier ones. */
	args(args: Readonly<Record<string, QueryValue>>): this {
		assertPlainObject(args, 'args');
		this.#args = { ...this.#args, ...cloneJson(args) };
		return this;
	}

	/**
	 * Hasura-style `order_by` entries. Direction strings become GraphQL enums.
	 * Pass multiple objects for multi-key order.
	 */
	orderBy(
		...entries: ReadonlyArray<Readonly<Record<string, OrderDirection | QueryEnumValue | QueryVariableRef>>>
	): this {
		if (entries.length === 0) {
			throw new TypeError('orderBy requires at least one order entry');
		}
		const orderBy = entries.map((entry) => {
			assertPlainObject(entry, 'orderBy entry');
			const mapped: Record<string, QueryValue> = {};
			for (const [field, direction] of Object.entries(entry)) {
				assertIdent(field, 'order field');
				if (typeof direction === 'string') {
					if (direction !== 'asc' && direction !== 'desc') {
						throw new TypeError(
							`orderBy direction for \`${field}\` must be "asc" or "desc"`
						);
					}
					mapped[field] = gqlEnum(direction);
				} else if (isQueryEnumValue(direction) || isQueryVariableRef(direction)) {
					mapped[field] = direction;
				} else {
					throw new TypeError(
						`orderBy direction for \`${field}\` must be "asc", "desc", gqlEnum(), or gqlVar()`
					);
				}
			}
			return Object.freeze(mapped);
		});
		this.#args = { ...this.#args, order_by: Object.freeze(orderBy) };
		return this;
	}

	/** Set the `where` argument. */
	where(expression: QueryValue): this {
		this.#args = { ...this.#args, where: cloneJson(expression) };
		return this;
	}

	/** Set the `limit` argument (literal or variable ref). */
	limit(value: number | QueryVariableRef): this {
		if (typeof value === 'number') {
			if (!Number.isInteger(value) || value < 0) {
				throw new TypeError('limit must be a non-negative integer');
			}
			this.#args = { ...this.#args, limit: value };
			return this;
		}
		if (!isQueryVariableRef(value)) {
			throw new TypeError('limit must be a number or gqlVar()');
		}
		this.#args = { ...this.#args, limit: value };
		return this;
	}

	/** Set the `offset` argument (literal or variable ref). */
	offset(value: number | QueryVariableRef): this {
		if (typeof value === 'number') {
			if (!Number.isInteger(value) || value < 0) {
				throw new TypeError('offset must be a non-negative integer');
			}
			this.#args = { ...this.#args, offset: value };
			return this;
		}
		if (!isQueryVariableRef(value)) {
			throw new TypeError('offset must be a number or gqlVar()');
		}
		this.#args = { ...this.#args, offset: value };
		return this;
	}

	/** Field selection tree. */
	select(selection: QuerySelection): this {
		assertSelection(selection, 'select');
		this.#select = cloneJson(selection);
		return this;
	}

	/** Mark the operation for SSR `@load` route registration. */
	load(): this {
		this.#load = true;
		return this;
	}

	/** Mark the operation for live companion generation (`@live`). */
	live(): this {
		this.#live = true;
		return this;
	}

	/** Freeze a portable QuerySpec object. */
	build(): QuerySpec {
		if (this.#field === undefined) {
			throw new TypeError('QueryBuilder requires from(field) before build()');
		}
		if (this.#select === undefined) {
			throw new TypeError('QueryBuilder requires select(...) before build()');
		}
		const root: QuerySpecRoot = {
			field: this.#field,
			select: this.#select,
			...(this.#alias !== undefined ? { alias: this.#alias } : {}),
			...(Object.keys(this.#args).length > 0
				? { args: Object.freeze({ ...this.#args }) }
				: {})
		};
		return Object.freeze({
			version: QUERY_SPEC_VERSION,
			name: this.#name,
			...(this.#load ? { load: true } : {}),
			...(this.#live ? { live: true } : {}),
			...(this.#variables.length > 0
				? { variables: Object.freeze([...this.#variables]) }
				: {}),
			roots: Object.freeze([Object.freeze(root)])
		});
	}

	/** Serialize QuerySpec as stable JSON for `*.query.json` files. */
	toJSON(space: number | string = 2): string {
		return `${JSON.stringify(this.build(), null, space)}\n`;
	}

	/**
	 * Lower to GraphQL document text.
	 *
	 * Prefer committing `*.query.json` and letting `dctl client` lower, so the
	 * Rust compiler remains the single source of GraphQL canonicalization.
	 * This helper is for tests, previews, and tooling.
	 */
	toGraphql(): string {
		return lowerQuerySpecToGraphql(this.build());
	}
}

/** Lower a QuerySpec to GraphQL text (mirrors the Rust client compiler). */
export function lowerQuerySpecToGraphql(spec: QuerySpec): string {
	if (spec.version !== QUERY_SPEC_VERSION) {
		throw new TypeError(
			`QuerySpec version ${String(spec.version)} is unsupported; expected ${QUERY_SPEC_VERSION}`
		);
	}
	assertOperationName(spec.name);
	if (!Array.isArray(spec.roots) || spec.roots.length !== 1) {
		throw new TypeError('QuerySpec v1 requires exactly one root');
	}

	let directives = '';
	if (spec.load) directives += ' @load';
	if (spec.live) directives += ' @live';

	const variables = spec.variables ?? [];
	const variableDefinitions =
		variables.length === 0
			? ''
			: `(${variables
					.map((variable) => {
						assertIdent(variable.name, 'variable name');
						assertGraphqlType(variable.type);
						return `$${variable.name}: ${variable.type}`;
					})
					.join(', ')})`;

	const root = spec.roots[0]!;
	assertIdent(root.field, 'root field');
	const head =
		root.alias !== undefined && root.alias !== root.field
			? `${assertIdent(root.alias, 'root alias')}: ${root.field}`
			: root.field;
	const args = renderArguments(root.args ?? {});
	// Root field is indented 2 spaces; its selection members are one level deeper.
	const selection = renderSelection(root.select, 2);

	return `query ${spec.name}${variableDefinitions}${directives} {\n  ${head}${args} {\n${selection}  }\n}\n`;
}

function renderArguments(args: Readonly<Record<string, QueryValue>>): string {
	const keys = Object.keys(args);
	if (keys.length === 0) return '';
	const parts = keys.map((name) => {
		assertIdent(name, 'argument');
		return `${name}: ${renderValue(args[name]!)}`;
	});
	return `(${parts.join(', ')})`;
}

function renderSelection(selection: QuerySelection, indent: number): string {
	assertSelection(selection, 'select');
	const pad = '  '.repeat(indent);
	const lines: string[] = [];
	for (const [field, value] of Object.entries(selection)) {
		assertIdent(field, 'selection field');
		if (value === true) {
			lines.push(`${pad}${field}`);
		} else {
			const nested = renderSelection(value, indent + 1);
			lines.push(`${pad}${field} {\n${nested}${pad}}`);
		}
	}
	return lines.map((line) => `${line}\n`).join('');
}

function renderValue(value: QueryValue): string {
	if (value === null) return 'null';
	if (typeof value === 'boolean') return value ? 'true' : 'false';
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) {
			throw new TypeError('QuerySpec numbers must be finite');
		}
		return String(value);
	}
	if (typeof value === 'string') {
		return `"${escapeGraphqlString(value)}"`;
	}
	if (Array.isArray(value)) {
		return `[${value.map((item) => renderValue(item as QueryValue)).join(', ')}]`;
	}
	if (isQueryEnumValue(value)) {
		assertIdent(value.$enum, 'enum value');
		return value.$enum;
	}
	if (isQueryVariableRef(value)) {
		assertIdent(value.$var, 'variable reference');
		return `$${value.$var}`;
	}
	if (typeof value === 'object') {
		const keys = Object.keys(value);
		const parts = keys.map((key) => {
			if (key.startsWith('$')) {
				throw new TypeError(
					`unknown QuerySpec value tag \`${key}\`; supported tags are $enum and $var`
				);
			}
			assertIdent(key, 'input field');
			return `${key}: ${renderValue((value as Record<string, QueryValue>)[key]!)}`;
		});
		return `{${parts.join(', ')}}`;
	}
	throw new TypeError('unsupported QuerySpec value');
}

function escapeGraphqlString(value: string): string {
	return value
		.replaceAll('\\', '\\\\')
		.replaceAll('"', '\\"')
		.replaceAll('\n', '\\n')
		.replaceAll('\r', '\\r')
		.replaceAll('\t', '\\t');
}

function assertOperationName(name: string): string {
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
		throw new TypeError(`QuerySpec name \`${name}\` is not a valid GraphQL name`);
	}
	return name;
}

function assertIdent(name: string, kind: string): string {
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
		throw new TypeError(`QuerySpec ${kind} \`${name}\` is not a valid GraphQL name`);
	}
	return name;
}

function assertGraphqlType(type: string): void {
	if (type.trim().length === 0) {
		throw new TypeError('QuerySpec variable type must be non-empty');
	}
	if (!/^[A-Za-z0-9_!\[\] ]+$/.test(type)) {
		throw new TypeError(
			`QuerySpec variable type \`${type}\` contains unsupported characters`
		);
	}
}

function assertPlainObject(value: unknown, label: string): asserts value is Record<string, unknown> {
	if (value === null || typeof value !== 'object' || Array.isArray(value)) {
		throw new TypeError(`${label} must be a plain object`);
	}
}

function assertSelection(value: unknown, label: string): asserts value is QuerySelection {
	assertPlainObject(value, label);
	const entries = Object.entries(value);
	if (entries.length === 0) {
		throw new TypeError(`${label} must include at least one field`);
	}
	for (const [field, selection] of entries) {
		assertIdent(field, 'selection field');
		if (selection === true) continue;
		if (selection === false) {
			throw new TypeError(
				`QuerySpec field \`${field}\` is false; omit excluded fields instead of setting false`
			);
		}
		assertSelection(selection, `select.${field}`);
	}
}

function cloneJson<T>(value: T): T {
	return JSON.parse(JSON.stringify(value)) as T;
}
