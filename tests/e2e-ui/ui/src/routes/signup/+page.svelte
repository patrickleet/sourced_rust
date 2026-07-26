<script lang="ts">
	import type { ActionData, PageData } from './$types';

	let { data, form }: { data: PageData; form: ActionData } = $props();

	const error = $derived(form?.error);
	const username = $derived(form?.username ?? '');
	const email = $derived(form?.email ?? '');
	const givenName = $derived(form?.givenName ?? '');
	const familyName = $derived(form?.familyName ?? '');
	const loginHref = $derived(
		`/login?authRequest=${encodeURIComponent(data.authRequest ?? '')}`
	);
</script>

<svelte:head>
	<title>Create account · e2e-ui</title>
</svelte:head>

<main class="auth-page">
	<div class="auth-card">
		<div class="auth-brand">
			<span class="auth-mark" aria-hidden="true">df</span>
			<span class="auth-product">e2e-ui</span>
		</div>
		<h1 class="auth-title">Create account</h1>
		<p class="auth-lead">
			Register on Fieldnote. Your password is verified via Zitadel Session API; Auth.js still
			holds the OIDC session cookie.
		</p>

		{#if error}
			<p class="auth-error" role="alert">{error}</p>
		{/if}

		<form method="POST" class="auth-form">
			<input type="hidden" name="authRequest" value={data.authRequest} />

			<label class="auth-label" for="username">Username</label>
			<input
				id="username"
				name="username"
				type="text"
				autocomplete="username"
				required
				class="auth-input"
				value={username}
				placeholder="carol"
			/>

			<label class="auth-label" for="email">Email</label>
			<input
				id="email"
				name="email"
				type="email"
				autocomplete="email"
				required
				class="auth-input"
				value={email}
				placeholder="carol@example.com"
			/>

			<div class="auth-row">
				<div class="auth-col">
					<label class="auth-label" for="givenName">First name</label>
					<input
						id="givenName"
						name="givenName"
						type="text"
						autocomplete="given-name"
						class="auth-input"
						value={givenName}
					/>
				</div>
				<div class="auth-col">
					<label class="auth-label" for="familyName">Last name</label>
					<input
						id="familyName"
						name="familyName"
						type="text"
						autocomplete="family-name"
						class="auth-input"
						value={familyName}
					/>
				</div>
			</div>

			<label class="auth-label" for="password">Password</label>
			<input
				id="password"
				name="password"
				type="password"
				autocomplete="new-password"
				required
				minlength="8"
				class="auth-input"
				placeholder="At least 8 characters"
			/>

			<button type="submit" class="auth-submit">Create account</button>
		</form>

		<p class="auth-meta">
			<a href={loginHref}>Already have an account?</a>
			<span class="auth-dot" aria-hidden="true">·</span>
			<a href="/">Back home</a>
		</p>
	</div>
</main>

<style>
	.auth-page {
		min-height: calc(100vh - 4rem);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 2rem var(--wf-gutter, 1.5rem);
		background: var(--wf-bg, #f6f5f2);
	}
	.auth-card {
		width: min(100%, 26rem);
		background: var(--wf-bg-elevated, #fff);
		border: 1px solid var(--wf-line, #e2e0d9);
		border-radius: 10px;
		padding: 2rem 1.75rem 1.75rem;
		box-shadow: 0 12px 32px rgba(28, 28, 26, 0.06);
	}
	.auth-brand {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-bottom: 1.25rem;
	}
	.auth-mark {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.75rem;
		height: 1.75rem;
		border-radius: 5px;
		background: var(--wf-ink, #1c1c1a);
		color: #f6f5f2;
		font-family: var(--wf-mono, ui-monospace, monospace);
		font-size: 0.7rem;
		font-weight: 500;
	}
	.auth-product {
		font-weight: 600;
		font-size: 0.95rem;
		color: var(--wf-ink, #1c1c1a);
	}
	.auth-title {
		margin: 0 0 0.35rem;
		font-family: var(--wf-serif, Georgia, serif);
		font-size: 1.65rem;
		font-weight: 500;
		letter-spacing: -0.02em;
		color: var(--wf-ink, #1c1c1a);
	}
	.auth-lead {
		margin: 0 0 1.25rem;
		font-size: 0.875rem;
		line-height: 1.45;
		color: var(--wf-ink-soft, #5c5c56);
	}
	.auth-error {
		margin: 0 0 1rem;
		padding: 0.65rem 0.75rem;
		border-radius: 6px;
		background: rgba(179, 58, 58, 0.08);
		border: 1px solid rgba(179, 58, 58, 0.25);
		color: var(--wf-danger, #b33a3a);
		font-size: 0.875rem;
	}
	.auth-form {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}
	.auth-row {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 0.75rem;
	}
	.auth-col {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
	}
	.auth-label {
		font-size: 0.8rem;
		font-weight: 500;
		color: var(--wf-ink-soft, #5c5c56);
		margin-top: 0.5rem;
	}
	.auth-input {
		appearance: none;
		border: 1px solid var(--wf-line-strong, #cdcabe);
		border-radius: 6px;
		padding: 0.6rem 0.75rem;
		font: inherit;
		font-size: 0.95rem;
		background: #fff;
		color: var(--wf-ink, #1c1c1a);
		width: 100%;
		box-sizing: border-box;
	}
	.auth-input:focus {
		outline: 2px solid var(--wf-accent, #3d5a80);
		outline-offset: 1px;
	}
	.auth-submit {
		margin-top: 1.15rem;
		appearance: none;
		border: none;
		border-radius: 6px;
		padding: 0.7rem 1rem;
		font: inherit;
		font-weight: 600;
		font-size: 0.95rem;
		background: var(--wf-accent, #3d5a80);
		color: #fff;
		cursor: pointer;
	}
	.auth-submit:hover {
		filter: brightness(1.06);
	}
	.auth-submit:active {
		transform: translateY(0.5px);
	}
	.auth-meta {
		margin: 1.25rem 0 0;
		font-size: 0.85rem;
		color: var(--wf-ink-soft, #5c5c56);
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		align-items: center;
	}
	.auth-meta a {
		color: var(--wf-accent, #3d5a80);
		text-decoration: none;
		font-weight: 500;
	}
	.auth-meta a:hover {
		text-decoration: underline;
	}
	.auth-dot {
		opacity: 0.5;
	}
</style>
