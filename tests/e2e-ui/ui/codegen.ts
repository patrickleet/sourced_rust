import type { CodegenConfig } from '@graphql-codegen/cli';

// Houdini-inspired: role schema + route query/subscription documents → typed documents.
// Registered command mutations are generated separately from the Rust manifest.
// Use admin SDL: field-superset of user (includes admin-only mutations like todos_force_archive).
// App still runs ops as the session role; user simply cannot execute admin-only fields at runtime.
const config: CodegenConfig = {
	schema: 'schema/admin.graphql',
	documents: ['src/routes/**/*.gql'],
	generates: {
		'src/lib/gql/generated/types.ts': {
			plugins: ['typescript']
		},
		'src/': {
			preset: 'near-operation-file',
			presetConfig: {
				extension: '.generated.ts',
				baseTypesPath: 'lib/gql/generated/types.ts',
				folder: ''
			},
			plugins: ['typescript-operations', 'typed-document-node'],
			config: {
				avoidOptionals: {
					field: true,
					inputValue: false,
					object: true,
					defaultValue: true
				},
				enumsAsTypes: true,
				skipTypename: true,
				documentMode: 'documentNode',
				useTypeImports: true
			}
		}
	}
};

export default config;
