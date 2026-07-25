/**
 * TypeScript query builder for Distributed client operations.
 *
 * Dual authoring surfaces:
 * - GraphQL documents (`.graphql`) — joins, GraphiQL, exploratory shapes
 * - This builder (`.query.ts`) — typed, model-shaped reads
 *
 * Both produce the same frozen client artifacts. The builder materializes to
 * **GraphQL document text** before `dctl client` runs. GraphQL remains the only
 * query language on the wire; there is no JSON query dialect.
 *
 * @example
 * ```ts
 * // src/routes/todos/+page.query.ts
 * import { defineQuery } from '@hops-ops/distributed/query';
 *
 * export default defineQuery('Todos')
 *   .from('todos')
 *   .orderBy({ status: 'asc' }, { todo_id: 'asc' })
 *   .select({ todo_id: true, title: true, status: true })
 *   .load();
 * ```
 */

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

export type OrderDirection = 'asc' | 'desc';

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

/** True when value is a GraphQL enum tag. */
export function isQueryEnumValue(value: unknown): value is QueryEnumValue {
	return (
		typeof value === 'object' &&
		value !== null &&
		Object.keys(value).length === 1 &&
		typeof (value as QueryEnumValue).$enum === 'string'
	);
}

/** True when value is a GraphQL variable tag. */
export function isQueryVariableRef(value: unknown): value is QueryVariableRef {
	return (
		typeof value === 'object' &&
		value !== null &&
		Object.keys(value).length === 1 &&
		typeof (value as QueryVariableRef).$var === 'string'
	);
}

/**
 * Start a named query builder. Default-export the builder from a `*.query.ts`
 * module; the SvelteKit Vite integration materializes `toGraphql()` for dctl.
 */
export function defineQuery(name: string): QueryBuilder {
	return new QueryBuilder(name);
}

export class QueryBuilder {
	readonly #name: string;
	#load = false;
	#live = false;
	#variables: Array<{ name: string; type: string }> = [];
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
		this.#variables.push({ name, type });
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
		...entries: ReadonlyArray<
			Readonly<Record<string, OrderDirection | QueryEnumValue | QueryVariableRef>>
		>
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

	/**
	 * Materialize the exact GraphQL document text compiled by `dctl client`.
	 * This is the builder's product — not a parallel query language.
	 */
	toGraphql(): string {
		if (this.#field === undefined) {
			throw new TypeError('QueryBuilder requires from(field) before toGraphql()');
		}
		if (this.#select === undefined) {
			throw new TypeError('QueryBuilder requires select(...) before toGraphql()');
		}

		let directives = '';
		if (this.#load) directives += ' @load';
		if (this.#live) directives += ' @live';

		const variableDefinitions =
			this.#variables.length === 0
				? ''
				: `(${this.#variables
						.map((variable) => `$${variable.name}: ${variable.type}`)
						.join(', ')})`;

		const head =
			this.#alias !== undefined && this.#alias !== this.#field
				? `${this.#alias}: ${this.#field}`
				: this.#field;
		const args = renderArguments(this.#args);
		const selection = renderSelection(this.#select, 2);

		return `query ${this.#name}${variableDefinitions}${directives} {\n  ${head}${args} {\n${selection}  }\n}\n`;
	}
}

export {
	evaluateQueryModule,
	materializeClientDocuments,
	type MaterializeClientDocumentsOptions,
	type MaterializedClientDocuments
} from './materialize.js';

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
			throw new TypeError('query values must be finite numbers');
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
					`unknown query value tag \`${key}\`; supported tags are $enum and $var`
				);
			}
			assertIdent(key, 'input field');
			return `${key}: ${renderValue((value as Record<string, QueryValue>)[key]!)}`;
		});
		return `{${parts.join(', ')}}`;
	}
	throw new TypeError('unsupported query value');
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
		throw new TypeError(`query name \`${name}\` is not a valid GraphQL name`);
	}
	return name;
}

function assertIdent(name: string, kind: string): string {
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
		throw new TypeError(`query ${kind} \`${name}\` is not a valid GraphQL name`);
	}
	return name;
}

function assertGraphqlType(type: string): void {
	if (type.trim().length === 0) {
		throw new TypeError('variable type must be non-empty');
	}
	if (!/^[A-Za-z0-9_!\[\] ]+$/.test(type)) {
		throw new TypeError(`variable type \`${type}\` contains unsupported characters`);
	}
}

function assertPlainObject(
	value: unknown,
	label: string
): asserts value is Record<string, unknown> {
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
				`field \`${field}\` is false; omit excluded fields instead of setting false`
			);
		}
		assertSelection(selection, `select.${field}`);
	}
}

function cloneJson<T>(value: T): T {
	return JSON.parse(JSON.stringify(value)) as T;
}
