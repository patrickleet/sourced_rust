<script lang="ts">
	import type { ActionData, PageData } from './$types';

	let { data, form }: { data: PageData; form: ActionData } = $props();

	const error = $derived(form?.error);
	const loginName = $derived(form?.loginName ?? '');
	const signupHref = $derived(
		`/signup?authRequest=${encodeURIComponent(data.authRequest ?? '')}`
	);
</script>

<svelte:head>
	<title>Sign in · e2e-ui</title>
</svelte:head>

<main class="auth-page">
	<div class="auth-card">
		<div class="auth-brand">
			<span class="auth-mark" aria-hidden="true">df</span>
			<span class="auth-product">e2e-ui</span>
		</div>
		<h1 class="auth-title">Sign in</h1>
		<p class="auth-lead">
			Your credentials stay on Fieldnote pages. Zitadel only issues the OIDC tokens after Auth.js
			completes the code flow.
		</p>

		{#if error}
			<p class="auth-error" role="alert">{error}</p>
		{/if}

		<form method="POST" class="auth-form">
			<input type="hidden" name="authRequest" value={data.authRequest} />

			<label class="auth-label" for="loginName">Username</label>
			<input
				id="loginName"
				name="loginName"
				type="text"
				autocomplete="username"
				required
				class="auth-input"
				value={loginName}
				placeholder="alice"
			/>

			<label class="auth-label" for="password">Password</label>
			<input
				id="password"
				name="password"
				type="password"
				autocomplete="current-password"
				required
				class="auth-input"
				placeholder="••••••••"
			/>

			<button type="submit" class="auth-submit">Continue</button>
		</form>

		<p class="auth-meta">
			<a href={signupHref}>Create an account</a>
			<span class="auth-dot" aria-hidden="true">·</span>
			<a href="/">Back home</a>
		</p>
		{#if data.demoHint}
			<p class="auth-demo">{data.demoHint}</p>
		{/if}
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
		width: min(100%, 24rem);
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

		&:focus {
			outline: 2px solid var(--wf-accent, #3d5a80);
			outline-offset: 1px;

		}
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

		&:hover {
			filter: brightness(1.06);

		}
		&:active {
			transform: translateY(0.5px);

		}
	}

	.auth-meta {
		margin: 1.25rem 0 0;
		font-size: 0.85rem;
		color: var(--wf-ink-soft, #5c5c56);
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		align-items: center;

		a {
			color: var(--wf-accent, #3d5a80);
			text-decoration: none;
			font-weight: 500;

		}
		a:hover {
			text-decoration: underline;

		}
	}

	.auth-dot {
		opacity: 0.5;

	}

	.auth-demo {
		margin: 1rem 0 0;
		font-size: 0.75rem;
		font-family: var(--wf-mono, ui-monospace, monospace);
		color: var(--wf-ink-muted, #8a8a82);

	}
</style>
