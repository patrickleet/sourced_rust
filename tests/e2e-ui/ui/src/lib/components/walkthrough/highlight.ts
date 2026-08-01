/**
 * Lightweight, dependency-free syntax tint for walkthrough samples.
 * Covers Rust / TypeScript / GraphQL / JSON-ish snippets well enough for teaching.
 * Not a full parser — prioritizes readability over perfect accuracy.
 */

const ESC: Record<string, string> = {
	'&': '&amp;',
	'<': '&lt;',
	'>': '&gt;',
	'"': '&quot;'
};

function escapeHtml(s: string): string {
	return s.replace(/[&<>"]/g, (c) => ESC[c] ?? c);
}

const KEYWORDS = new Set([
	// Rust
	'as',
	'async',
	'await',
	'break',
	'const',
	'continue',
	'crate',
	'dyn',
	'else',
	'enum',
	'extern',
	'false',
	'fn',
	'for',
	'if',
	'impl',
	'in',
	'let',
	'loop',
	'match',
	'mod',
	'move',
	'mut',
	'pub',
	'ref',
	'return',
	'self',
	'Self',
	'static',
	'struct',
	'super',
	'trait',
	'true',
	'type',
	'unsafe',
	'use',
	'where',
	'while',
	// TS / JS
	'class',
	'constructor',
	'default',
	'export',
	'extends',
	'from',
	'function',
	'import',
	'interface',
	'new',
	'null',
	'of',
	'private',
	'protected',
	'public',
	'readonly',
	'throw',
	'try',
	'catch',
	'typeof',
	'undefined',
	'var',
	'void',
	'yield',
	// GraphQL
	'query',
	'mutation',
	'subscription',
	'fragment',
	'on',
	'schema',
	'input',
	'union',
	'scalar',
	'directive'
]);

const TYPES = new Set([
	'Result',
	'Option',
	'String',
	'Ok',
	'Err',
	'Some',
	'None',
	'Vec',
	'HashMap',
	'HandlerError',
	'Causal',
	'Projected',
	'PreparedCommand',
	'TodoError',
	'BlobError',
	'TodoState',
	'Todos',
	'BlobGames',
	'AuthUsers',
	'ModelPermissions',
	'Session',
	'Promise',
	'Record',
	'Array',
	'Map',
	'boolean',
	'number',
	'string',
	'unknown',
	'never'
]);

type Kind = 'comment' | 'string' | 'number' | 'keyword' | 'type' | 'fn' | 'attr' | 'punct' | 'plain';

function span(kind: Kind, text: string): string {
	if (kind === 'plain') return escapeHtml(text);
	return `<span class="tok-${kind}">${escapeHtml(text)}</span>`;
}

/**
 * Highlight source into HTML spans (safe-escaped).
 */
export function highlightCode(source: string): string {
	const out: string[] = [];
	let i = 0;
	const n = source.length;

	while (i < n) {
		const c = source[i];

		// Line comment //
		if (c === '/' && source[i + 1] === '/') {
			const end = source.indexOf('\n', i);
			const endIdx = end === -1 ? n : end;
			out.push(span('comment', source.slice(i, endIdx)));
			i = endIdx;
			continue;
		}

		// Block comment /* */
		if (c === '/' && source[i + 1] === '*') {
			const end = source.indexOf('*/', i + 2);
			const endIdx = end === -1 ? n : end + 2;
			out.push(span('comment', source.slice(i, endIdx)));
			i = endIdx;
			continue;
		}

		// GraphQL / shell-style # comment (not Rust raw strings)
		if (c === '#' && (i === 0 || source[i - 1] === '\n' || source[i - 1] === ' ')) {
			const end = source.indexOf('\n', i);
			const endIdx = end === -1 ? n : end;
			out.push(span('comment', source.slice(i, endIdx)));
			i = endIdx;
			continue;
		}

		// Attribute #[…] or @load
		if (c === '#' && source[i + 1] === '[') {
			let j = i + 2;
			let depth = 1;
			while (j < n && depth > 0) {
				if (source[j] === '[') depth++;
				else if (source[j] === ']') depth--;
				j++;
			}
			out.push(span('attr', source.slice(i, j)));
			i = j;
			continue;
		}
		if (c === '@') {
			let j = i + 1;
			while (j < n && /[A-Za-z0-9_]/.test(source[j])) j++;
			out.push(span('attr', source.slice(i, j)));
			i = j;
			continue;
		}

		// Rust lifetime / label: 'a, '_, 'static — must not be treated as a string.
		// (Was breaking `&CausalCommandContext<'_, Todo>` by swallowing to the next '.)
		if (c === "'" && i + 1 < n && /[A-Za-z_]/.test(source[i + 1]!)) {
			let j = i + 1;
			while (j < n && /[A-Za-z0-9_]/.test(source[j]!)) j++;
			out.push(span('attr', source.slice(i, j)));
			i = j;
			continue;
		}

		// Rust char literal: 'x', '\n', '\''
		if (c === "'" && i + 1 < n) {
			let j = i + 1;
			if (source[j] === '\\' && j + 1 < n) {
				j += 2; // escaped char
			} else if (source[j] !== "'") {
				j += 1; // single char
			}
			if (j < n && source[j] === "'") {
				j += 1;
				out.push(span('string', source.slice(i, j)));
				i = j;
				continue;
			}
			// Lone ' — emit as punct, don't eat the line
			out.push(span('punct', "'"));
			i = i + 1;
			continue;
		}

		// Double-quoted strings (and JS single-quoted only via char-literal path above)
		if (c === '"') {
			let j = i + 1;
			while (j < n) {
				if (source[j] === '\\') {
					j += 2;
					continue;
				}
				if (source[j] === '"') {
					j++;
					break;
				}
				j++;
			}
			out.push(span('string', source.slice(i, j)));
			i = j;
			continue;
		}
		// Template / backtick
		if (c === '`') {
			let j = i + 1;
			while (j < n) {
				if (source[j] === '\\') {
					j += 2;
					continue;
				}
				if (source[j] === '`') {
					j++;
					break;
				}
				j++;
			}
			out.push(span('string', source.slice(i, j)));
			i = j;
			continue;
		}

		// Numbers
		if (/[0-9]/.test(c) && (i === 0 || !/[A-Za-z_$]/.test(source[i - 1]))) {
			let j = i;
			while (j < n && /[0-9.xXa-fA-F_]/.test(source[j])) j++;
			out.push(span('number', source.slice(i, j)));
			i = j;
			continue;
		}

		// Identifiers
		if (/[A-Za-z_$]/.test(c)) {
			let j = i + 1;
			while (j < n && /[A-Za-z0-9_$]/.test(source[j])) j++;
			const word = source.slice(i, j);
			// function name: ident followed by (
			let k = j;
			while (k < n && (source[k] === ' ' || source[k] === '\t')) k++;
			if (source[k] === '(' && !KEYWORDS.has(word)) {
				out.push(span('fn', word));
			} else if (KEYWORDS.has(word)) {
				out.push(span('keyword', word));
			} else if (TYPES.has(word) || /^[A-Z]/.test(word)) {
				out.push(span('type', word));
			} else {
				out.push(span('plain', word));
			}
			i = j;
			continue;
		}

		// Punctuation / operators
		if (/[{}()\[\];,.:?<>=!&|+\-*/%~^]/.test(c)) {
			let j = i + 1;
			// multi-char ops
			const two = source.slice(i, i + 2);
			if (
				[
					'=>',
					'->',
					'::',
					'==',
					'!=',
					'<=',
					'>=',
					'&&',
					'||',
					'+=',
					'-=',
					'*=',
					'/=',
					'?.',
					'??',
					'...'
				].includes(two) ||
				(two === '..' && source[i + 2] !== '.')
			) {
				j = i + 2;
			}
			if (source.slice(i, i + 3) === '...') j = i + 3;
			out.push(span('punct', source.slice(i, j)));
			i = j;
			continue;
		}

		out.push(escapeHtml(c));
		i++;
	}

	return out.join('');
}
