<script lang="ts">
	import { Code2, Eye, EyeOff } from '@lucide/svelte';

	interface Props {
		label: string;
		token?: string | null;
		present?: boolean;
		statusLabel?: string;
		hiddenMessage?: string;
		protectedMessage?: string;
	}

	interface DecodedJwt {
		header: string;
		payload: string;
		signatureSummary: string;
	}

	type DecodeResult =
		| { ok: true; decoded: DecodedJwt }
		| { ok: false; message: string };
	type JsonPartResult =
		| { ok: true; json: string }
		| { ok: false; message: string };

	let {
		label,
		token = '',
		present = false,
		statusLabel,
		hiddenMessage = 'Available to the app. Hidden in the UI until revealed.',
		protectedMessage = ''
	}: Props = $props();

	let revealed = $state(false);
	let decodedOpen = $state(false);

	const tokenValue = $derived(typeof token === 'string' ? token.trim() : '');
	const hasToken = $derived(tokenValue.length > 0);
	const status = $derived(statusLabel ?? (hasToken ? 'Present' : present ? 'Protected' : 'Missing'));
	const statusClass = $derived(hasToken || present ? 'present' : 'missing');
	const revealLabel = $derived(revealed ? 'Hide token' : 'Show token');
	const decodeLabel = $derived(decodedOpen ? 'Hide decoded' : 'Decode');
	const decoded = $derived(
		hasToken && revealed && decodedOpen ? decodeJwt(tokenValue) : null
	);

	function toggleRevealed() {
		revealed = !revealed;
		if (!revealed) decodedOpen = false;
	}

	function decodeJwt(value: string): DecodeResult {
		const parts = value.split('.');
		if (parts.length !== 3) {
			return {
				ok: false,
				message: 'Expected a JWT with header, payload, and signature.'
			};
		}

		const header = decodeJwtJsonPart(parts[0], 'header');
		if (!header.ok) return header;

		const payload = decodeJwtJsonPart(parts[1], 'payload');
		if (!payload.ok) return payload;

		return {
			ok: true,
			decoded: {
				header: header.json,
				payload: payload.json,
				signatureSummary: parts[2]
					? `Signature: ${parts[2].length} base64url characters`
					: 'Signature: empty'
			}
		};
	}

	function decodeJwtJsonPart(part: string, name: string): JsonPartResult {
		const decoded = decodeBase64Url(part);
		if (!decoded.ok) {
			return {
				ok: false,
				message: `Could not decode JWT ${name}: ${decoded.message}`
			};
		}

		try {
			const json = JSON.parse(new TextDecoder().decode(decoded.bytes));
			return { ok: true, json: JSON.stringify(json, null, 2) };
		} catch {
			return {
				ok: false,
				message: `JWT ${name} is not valid JSON.`
			};
		}
	}

	function decodeBase64Url(input: string):
		| { ok: true; bytes: Uint8Array }
		| { ok: false; message: string } {
		const unpadded = input.trimEnd().replace(/=+$/, '');

		if (/=/.test(unpadded)) {
			return { ok: false, message: 'padding must be at the end' };
		}

		if (!/^[A-Za-z0-9_-]*$/.test(unpadded)) {
			return { ok: false, message: 'invalid base64url character' };
		}

		if (unpadded.length % 4 === 1) {
			return { ok: false, message: 'invalid base64url length' };
		}

		const normalized = unpadded.replace(/-/g, '+').replace(/_/g, '/');
		const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=');

		try {
			const binary = globalThis.atob(padded);
			return {
				ok: true,
				bytes: Uint8Array.from(binary, (char) => char.charCodeAt(0))
			};
		} catch {
			return { ok: false, message: 'invalid base64url data' };
		}
	}
</script>

<article class="token-card">
	<div class="token-card-header">
		<div class="token-title-block">
			<span class="token-label">{label}</span>
			<strong class="token-status {statusClass}">{status}</strong>
		</div>

		<div class="token-actions">
			{#if hasToken}
				<button
					class="token-icon-button"
					type="button"
					aria-label={revealLabel}
					title={revealLabel}
					onclick={toggleRevealed}
				>
					{#if revealed}
						<EyeOff size={18} strokeWidth={2.2} aria-hidden="true" />
					{:else}
						<Eye size={18} strokeWidth={2.2} aria-hidden="true" />
					{/if}
				</button>

				{#if revealed}
					<button
						class="token-decode-button"
						type="button"
						onclick={() => {
							decodedOpen = !decodedOpen;
						}}
					>
						<Code2 size={17} strokeWidth={2.2} aria-hidden="true" />
						<span>{decodeLabel}</span>
					</button>
				{/if}
			{/if}
		</div>
	</div>

	{#if hasToken && revealed}
		<div class="token-reveal">
			<pre class="token-code"><code>{tokenValue}</code></pre>
		</div>

		{#if decoded}
			{#if decoded.ok}
				<div class="token-decoded">
					<div class="token-decoded-grid">
						<div>
							<span class="token-json-label">Header</span>
							<pre class="token-json"><code>{decoded.decoded.header}</code></pre>
						</div>
						<div>
							<span class="token-json-label">Payload</span>
							<pre class="token-json"><code>{decoded.decoded.payload}</code></pre>
						</div>
					</div>
					<p class="token-meta">{decoded.decoded.signatureSummary}</p>
				</div>
			{:else}
				<p class="token-decode-error">{decoded.message}</p>
			{/if}
		{/if}
	{:else if present && protectedMessage}
		<p class="token-unavailable">{protectedMessage}</p>
	{:else if !hasToken}
		<p class="token-unavailable">Token is not present in the current session.</p>
	{:else}
		<p class="token-unavailable">{hiddenMessage}</p>
	{/if}
</article>

<style>
	.token-card {
		background: var(--hops-bg-white);
		border: 1px solid var(--hops-border);
		border-radius: 8px;
		box-shadow: var(--shadow-sm);
		overflow: hidden;
	}

	.token-card-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		min-height: 72px;
		padding: 1rem 1.1rem;
	}

	.token-title-block {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 0.65rem;
		min-width: 0;
	}

	.token-label {
		color: var(--hops-navy);
		font-size: 0.98rem;
		font-weight: 850;
	}

	.token-status {
		display: inline-flex;
		align-items: center;
		min-height: 26px;
		padding: 0.2rem 0.55rem;
		border-radius: 999px;
		font-size: 0.75rem;
		font-weight: 800;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.token-status.present {
		background: rgba(24, 121, 78, 0.11);
		border: 1px solid rgba(24, 121, 78, 0.26);
		color: #18794e;
	}

	.token-status.missing {
		background: rgba(180, 35, 24, 0.09);
		border: 1px solid rgba(180, 35, 24, 0.2);
		color: #b42318;
	}

	.token-actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		flex-wrap: wrap;
		gap: 0.45rem;
	}

	.token-icon-button,
	.token-decode-button {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border: 1px solid var(--hops-border);
		background: var(--hops-bg-light);
		color: var(--hops-navy);
		cursor: pointer;
		transition:
			background 0.2s ease,
			border-color 0.2s ease,
			color 0.2s ease;
	}

	.token-icon-button {
		width: 38px;
		height: 38px;
		border-radius: 8px;
	}

	.token-decode-button {
		min-height: 38px;
		gap: 0.45rem;
		padding: 0 0.75rem;
		border-radius: 8px;
		font-weight: 800;
	}

	.token-icon-button:hover,
	.token-decode-button:hover {
		background: var(--hops-navy);
		border-color: var(--hops-navy);
		color: var(--hops-text-inverse);
	}

	.token-reveal,
	.token-decoded {
		border-top: 1px solid var(--hops-border);
		background: #f8fafc;
	}

	.token-reveal {
		padding: 0.9rem 1.1rem;
		background: var(--hops-navy);
	}

	.token-code,
	.token-json {
		margin: 0;
		overflow-x: auto;
		white-space: pre-wrap;
		font-family: var(--font-mono);
		font-size: 0.86rem;
		line-height: 1.7;
		overflow-wrap: anywhere;
	}

	.token-code :global(code),
	.token-json :global(code) {
		display: block;
		padding: 0 !important;
		background: transparent !important;
		color: inherit !important;
		border-radius: 0 !important;
		font: inherit;
	}

	.token-code {
		background: transparent !important;
		border-radius: 0 !important;
		color: #f8fafc;
		overflow: visible;
		word-break: break-all;
	}

	.token-code::after,
	.token-json::after {
		display: none !important;
	}

	.token-decoded {
		padding: 1rem 1.1rem;
	}

	.token-decoded-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 0.85rem;
	}

	.token-json-label {
		display: block;
		margin-bottom: 0.45rem;
		color: var(--hops-text-muted);
		font-size: 0.76rem;
		font-weight: 850;
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}

	.token-json {
		padding: 0.85rem;
		max-height: none;
		overflow-x: auto;
		overflow-y: visible;
		border: 1px solid rgba(20, 32, 58, 0.12);
		border-radius: 8px;
		background: var(--hops-navy);
		color: #f8fafc;
	}

	.token-meta,
	.token-unavailable,
	.token-decode-error {
		margin: 0;
		color: var(--hops-text-muted);
		font-size: 0.9rem;
		line-height: 1.5;
	}

	.token-meta {
		margin-top: 0.8rem;
		font-family: var(--font-mono);
	}

	.token-unavailable {
		padding: 0 1.1rem 1rem;
	}

	.token-decode-error {
		padding: 0.9rem 1.1rem 1rem;
		border-top: 1px solid var(--hops-border);
		color: #b42318;
		font-weight: 750;
	}

	@media (--tablet) {
		.token-decoded-grid {
			grid-template-columns: 1fr;
		}
	}

	@media (--mobile) {
		.token-card-header {
			align-items: flex-start;
			flex-direction: column;
		}

		.token-actions {
			width: 100%;
			justify-content: flex-start;
		}
	}
</style>
