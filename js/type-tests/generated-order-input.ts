import type { Operation_ScalarInputs_Variables } from '../../distributed_cli/tests/fixtures/generated-operation';

type OrderValue = NonNullable<Operation_ScalarInputs_Variables['order']>;
type OrderEntry = Exclude<OrderValue, readonly unknown[]>;

export const orderByPriority: OrderEntry = {
	priority: 'asc'
};

export const orderByIdThenPriority: readonly OrderEntry[] = [
	{ id: 'asc' },
	{ priority: 'desc_nulls_last' }
];

const multipleFieldsInOneOrderEntry = {
	id: 'asc',
	priority: 'desc'
} as const;

// @ts-expect-error Structural values with multiple known fields are ambiguous.
export const rejectedMultipleFields: OrderEntry = multipleFieldsInOneOrderEntry;

// @ts-expect-error An order entry cannot be empty.
export const emptyOrderEntry: OrderEntry = {};

export const unknownOrderField: OrderEntry = {
	// @ts-expect-error Only compiler-authorized model fields may be ordered.
	missing: 'asc'
};
