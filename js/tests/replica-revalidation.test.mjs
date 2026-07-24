import assert from 'node:assert/strict';
import test from 'node:test';

import { createReplicaRevalidationMatcher } from '../dist/replica/revalidation.js';
import { TodosArtifact } from './fixtures/adapter-conformance.mjs';

const relationshipArtifact = Object.freeze({
	...TodosArtifact,
	id: 'query:users-and-todos',
	roots: Object.freeze([
		Object.freeze({
			responseKey: 'users',
			field: 'users',
			cardinality: 'many',
			nullable: false,
			dependencies: Object.freeze(['users']),
			selection: Object.freeze({
				typename: 'UserView',
				storage: Object.freeze({
					kind: 'normalized',
					model: 'UserView',
					identityFields: Object.freeze(['id'])
				}),
				members: Object.freeze([
					Object.freeze({
						kind: 'branch',
						semantic: 'relationship',
						responseKey: 'todos',
						field: 'todos',
						cardinality: 'many',
						nullable: false,
						dependencies: Object.freeze(['user_todos']),
						relationship: Object.freeze({
							field: 'todos',
							targetModel: 'TodoView',
							kind: 'has_many',
							keyMapping: Object.freeze({
								kind: 'direct',
								local: Object.freeze(['id']),
								remote: Object.freeze(['owner_id'])
							}),
							maintenance: 'local',
							dependencies: Object.freeze(['users', 'todos'])
						}),
						selection: Object.freeze({
							typename: 'TodoView',
							storage: Object.freeze({
								kind: 'normalized',
								model: 'TodoView',
								identityFields: Object.freeze(['id'])
							}),
							members: Object.freeze([])
						})
					})
				])
			})
		})
	])
});

function plan(overrides = {}) {
	return {
		dependencies: [],
		models: [],
		relationships: [],
		...overrides
	};
}

test('revalidation matcher targets dependency, model, and exact relationship inventories', () => {
	assert.equal(
		createReplicaRevalidationMatcher(
			plan({ dependencies: ['todos'] })
		)(TodosArtifact),
		true
	);
	assert.equal(
		createReplicaRevalidationMatcher(
			plan({ dependencies: ['unrelated'] })
		)(TodosArtifact),
		false
	);
	assert.equal(
		createReplicaRevalidationMatcher(
			plan({ models: ['TodoView'] })
		)(TodosArtifact),
		true
	);
	assert.equal(
		createReplicaRevalidationMatcher(
			plan({ dependencies: ['user_todos'] })
		)(relationshipArtifact),
		true,
		'nested dependencies participate in targeting'
	);
	assert.equal(
		createReplicaRevalidationMatcher(
			plan({
				relationships: [
					{
						sourceModel: 'UserView',
						field: 'todos',
						targetModel: 'TodoView'
					}
				]
			})
		)(relationshipArtifact),
		true
	);
	assert.equal(
		createReplicaRevalidationMatcher(
			plan({
				relationships: [
					{
						sourceModel: 'OtherView',
						field: 'todos',
						targetModel: 'TodoView'
					}
				]
			})
		)(relationshipArtifact),
		false,
		'relationship identity is an exact source/field/target triple'
	);
});

test('empty revalidation inventory is conservative and malformed plans fail closed', () => {
	assert.equal(createReplicaRevalidationMatcher(plan())(TodosArtifact), true);
	assert.throws(
		() =>
			createReplicaRevalidationMatcher({
				dependencies: ['todos'],
				models: [],
				relationships: [{ sourceModel: '', field: 'todos', targetModel: 'TodoView' }]
			}),
		/replica revalidation plan relationships\[0\] is invalid/
	);
});
