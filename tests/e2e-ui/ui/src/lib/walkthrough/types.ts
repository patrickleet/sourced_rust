/** One code sample inside a walkthrough tab. */
export type WalkthroughSample = {
	/** Path-like label (e.g. routes/chat/+page.graphql) */
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
	/** What this layer is for (1–2 sentences) */
	lede: string;
	/** Distributed principle this tab exercises */
	principle: string;
	samples: WalkthroughSample[];
};

/** Full walkthrough for one demo route. */
export type DemoWalkthrough = {
	/** Stable key (chat | todos | blob | admin | session | public) */
	id: string;
	/** Route path */
	href: string;
	/** Panel title */
	title: string;
	/** One-line kicker */
	kicker: string;
	/** Opening blurb */
	summary: string;
	tabs: WalkthroughTab[];
};
