/** One code sample inside a walkthrough tab. */
export type WalkthroughSample = {
	/** Path-like label (e.g. routes/chat/+layout.graphql) */
	file: string;
	/** Short caption under the path */
	caption?: string;
	/** Source shown in the panel */
	code: string;
};

/** One tab in the How-it's-built drawer. */
export type WalkthroughTab = {
	/** Short tab label */
	id: string;
	label: string;
	/** What this layer does, what you write, and what you must not do. ASD-STE100. */
	lede: string;
	/** One teaching rule for this layer. ASD-STE100. */
	principle: string;
	samples: WalkthroughSample[];
};

/** Full walkthrough for one demo route. */
export type DemoWalkthrough = {
	/** Stable key (chat | todos | blob | admin | session) */
	id: string;
	/** Route path */
	href: string;
	/** Panel title */
	title: string;
	/** Short sequence of layers for this demo */
	kicker: string;
	/** Opening facts for a developer or agent. ASD-STE100. */
	summary: string;
	tabs: WalkthroughTab[];
};
