import type { CodegenConfig } from '@graphql-codegen/cli';

// Houdini-inspired: schema + co-located *.gql → *.generated.ts next to each .gql
const config: CodegenConfig = {
	schema: 'schema/user.graphql',
	documents: ['src/**/*.gql'],
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
				documentMode: 'documentNode'
			}
		}
	}
};

export default config;
